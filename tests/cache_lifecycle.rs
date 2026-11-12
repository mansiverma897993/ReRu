// SPDX-License-Identifier: AGPL-3.0-only
#[path = "common/client.rs"]
#[allow(dead_code)]
mod client;
#[allow(dead_code)]
mod common;

use client::{integer, ok, Client, Reply};
use common::Server;
use std::time::{Duration, Instant};

fn metric(c: &mut Client, section: &[u8], name: &str) -> u64 {
    let Reply::Bulk(Some(bytes)) = c.call(&[b"INFO", section]) else {
        panic!("expected INFO bulk reply")
    };
    String::from_utf8(bytes)
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix(&format!("{name}:")))
        .unwrap()
        .parse()
        .unwrap()
}

fn eventually(mut check: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !check() {
        assert!(Instant::now() < deadline, "condition did not become true");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn idle_keys_are_physically_reclaimed_across_multiple_batches() {
    let server = Server::start();
    let mut c = Client::new(&server);
    c.call(&[b"SET", b"persistent", b"keep"]);
    for i in 0..600 {
        assert_eq!(
            c.call(&[b"SET", format!("key-{i}").as_bytes(), b"v", b"PX", b"300"]),
            ok()
        );
    }
    // INFO observes allocated entries without reading/removing the expiring keys.
    eventually(|| metric(&mut c, b"memory", "reru_retained_keys") == 1);
    assert_eq!(metric(&mut c, b"memory", "reru_expiry_index_keys"), 0);
    assert_eq!(metric(&mut c, b"stats", "reru_active_expired_keys"), 600);
    assert_eq!(
        c.call(&[b"GET", b"persistent"]),
        Reply::Bulk(Some(b"keep".to_vec()))
    );
}

#[test]
fn updated_cancelled_and_replaced_ttls_do_not_delete_new_values() {
    let server = Server::start();
    let mut c = Client::new(&server);
    for key in [b"extend".as_slice(), b"persist", b"overwrite", b"recreate"] {
        c.call(&[b"SET", key, b"old", b"PX", b"1000"]);
    }
    assert_eq!(c.call(&[b"PEXPIRE", b"extend", b"60000"]), integer(1));
    assert_eq!(c.call(&[b"PERSIST", b"persist"]), integer(1));
    assert_eq!(c.call(&[b"SET", b"overwrite", b"new"]), ok());
    assert_eq!(c.call(&[b"DEL", b"recreate"]), integer(1));
    assert_eq!(c.call(&[b"SET", b"recreate", b"new"]), ok());
    c.call(&[b"SET", b"sentinel", b"v", b"PX", b"1200"]);
    eventually(|| metric(&mut c, b"stats", "reru_active_expired_keys") == 1);
    assert_eq!(metric(&mut c, b"memory", "reru_retained_keys"), 4);
    assert_eq!(metric(&mut c, b"memory", "reru_expiry_index_keys"), 1);
    assert_eq!(c.call(&[b"PTTL", b"persist"]), integer(-1));
    assert_eq!(
        c.call(&[b"GET", b"recreate"]),
        Reply::Bulk(Some(b"new".to_vec()))
    );
}

#[test]
fn expiry_invalidates_watch_and_memory_rollback_preserves_deadline() {
    let server = Server::start();
    let mut c = Client::new(&server);
    let mut watcher = Client::new(&server);
    c.call(&[b"SET", b"key", b"v", b"PX", b"1000"]);
    assert_eq!(watcher.call(&[b"WATCH", b"key"]), ok());
    assert_eq!(c.call(&[b"CONFIG", b"SET", b"maxmemory", b"100"]), ok());
    assert!(matches!(
        c.call(&[b"SET", b"key", &[b'x'; 1000]]),
        Reply::Error(_)
    ));
    assert_eq!(metric(&mut c, b"memory", "reru_expiry_index_keys"), 1);
    eventually(|| metric(&mut c, b"memory", "reru_retained_keys") == 0);
    watcher.call(&[b"MULTI"]);
    watcher.call(&[b"GET", b"key"]);
    assert_eq!(watcher.call(&[b"EXEC"]), Reply::Array(None));
}

#[test]
fn idle_expiry_is_persisted_and_startup_rebuilds_deadlines() {
    for mode in ["--appendonly", "--snapshot"] {
        let path = common::temp_path("reru-expiry", "");
        let filename = path.to_str().unwrap();
        {
            let server = Server::start_with(&[mode, filename]);
            let mut c = Client::new(&server);
            c.call(&[b"SET", b"keep", b"v"]);
            c.call(&[b"SET", b"expire-after-restart", b"v", b"PX", b"1500"]);
        }
        {
            let server = Server::start_with(&[mode, filename]);
            let mut c = Client::new(&server);
            assert_eq!(metric(&mut c, b"memory", "reru_expiry_index_keys"), 1);
            let before = std::fs::read(&path).unwrap();
            eventually(|| metric(&mut c, b"memory", "reru_retained_keys") == 1);
            eventually(|| std::fs::read(&path).unwrap() != before);
            if mode == "--appendonly" {
                let bytes = std::fs::read(&path).unwrap();
                let deletion = b"$3\r\nDEL\r\n$20\r\nexpire-after-restart\r\n";
                assert!(bytes[before.len()..]
                    .windows(deletion.len())
                    .any(|w| w == deletion));
            }
        }
        {
            let server = Server::start_with(&[mode, filename]);
            let mut c = Client::new(&server);
            assert_eq!(metric(&mut c, b"memory", "reru_retained_keys"), 1);
            assert_eq!(metric(&mut c, b"memory", "reru_expiry_index_keys"), 0);
        }
        std::fs::remove_file(path).unwrap();
    }
}

#[test]
fn primary_propagates_idle_expiry_to_replica() {
    let primary = Server::start();
    let mut writer = Client::new(&primary);
    writer.call(&[b"SET", b"key", b"v", b"PX", b"1500"]);
    let replica = Server::start_with(&["--replicaof", "127.0.0.1", &primary.port.to_string()]);
    let mut reader = Client::new(&replica);
    eventually(|| metric(&mut reader, b"memory", "reru_retained_keys") == 1);
    eventually(|| metric(&mut reader, b"memory", "reru_retained_keys") == 0);
    assert_eq!(metric(&mut writer, b"stats", "reru_active_expired_keys"), 1);
    assert_eq!(metric(&mut reader, b"stats", "reru_active_expired_keys"), 0);
}
