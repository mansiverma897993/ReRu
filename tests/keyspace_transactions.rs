// SPDX-License-Identifier: AGPL-3.0-only
#[path = "common/client.rs"]
mod client;
mod common;
use client::{array, integer, ok, queued, Client, Reply};
use std::time::Duration;
#[test]
fn key_existence_and_deletion_count_duplicates_correctly() {
    let server = common::Server::start();
    let mut client = Client::new(&server);
    assert_eq!(client.call(&[b"SET", b"binary\0key", b"value"]), ok());
    assert_eq!(client.call(&[b"RPUSH", b"list", b"a"]), integer(1));
    assert_eq!(
        client.call(&[
            b"EXISTS",
            b"binary\0key",
            b"binary\0key",
            b"list",
            b"missing"
        ]),
        integer(3)
    );
    assert_eq!(
        client.call(&[b"DEL", b"binary\0key", b"binary\0key", b"list", b"missing"]),
        integer(2)
    );
    assert_eq!(client.call(&[b"EXISTS", b"list"]), integer(0));
    assert_eq!(client.call(&[b"TTL", b"list"]), integer(-2));
}

#[test]
fn expiry_conditions_and_persist_work_for_all_value_types() {
    let server = common::Server::start();
    let mut client = Client::new(&server);
    assert_eq!(client.call(&[b"RPUSH", b"list", b"a"]), integer(1));
    assert_eq!(client.call(&[b"TTL", b"list"]), integer(-1));
    assert_eq!(client.call(&[b"PTTL", b"missing"]), integer(-2));
    assert_eq!(
        client.call(&[b"EXPIRE", b"list", b"100", b"XX"]),
        integer(0)
    );
    assert_eq!(
        client.call(&[b"EXPIRE", b"list", b"100", b"GT"]),
        integer(0)
    );
    assert_eq!(
        client.call(&[b"EXPIRE", b"list", b"100", b"NX"]),
        integer(1)
    );
    assert_eq!(client.call(&[b"TTL", b"list"]), integer(100));
    let Reply::Integer(ttl) = client.call(&[b"PTTL", b"list"]) else {
        panic!("expected integer")
    };
    assert!((99_000..=100_000).contains(&ttl));
    assert_eq!(client.call(&[b"EXPIRE", b"list", b"50", b"NX"]), integer(0));
    assert_eq!(client.call(&[b"EXPIRE", b"list", b"50", b"GT"]), integer(0));
    assert_eq!(
        client.call(&[b"EXPIRE", b"list", b"50", b"XX", b"LT"]),
        integer(1)
    );
    assert_eq!(client.call(&[b"PERSIST", b"list"]), integer(1));
    assert_eq!(client.call(&[b"PERSIST", b"list"]), integer(0));
    assert_eq!(client.call(&[b"TTL", b"list"]), integer(-1));
    assert_eq!(client.call(&[b"PEXPIRE", b"list", b"0"]), integer(1));
    assert_eq!(client.call(&[b"EXISTS", b"list"]), integer(0));
    assert_eq!(client.call(&[b"PERSIST", b"list"]), integer(0));
}

#[test]
fn expiry_rejects_invalid_numbers_flags_and_response_injection() {
    let server = common::Server::start();
    let mut client = Client::new(&server);
    assert_eq!(client.call(&[b"SET", b"k", b"v"]), ok());
    for number in [b"01".as_slice(), b"+1", b"1.5", b"9223372036854775808"] {
        assert!(matches!(
            client.call(&[b"EXPIRE", b"k", number]),
            Reply::Error(_)
        ));
    }
    assert!(matches!(
        client.call(&[b"EXPIRE", b"k", b"9223372036854775807"]),
        Reply::Error(_)
    ));
    assert!(matches!(
        client.call(&[b"EXPIRE", b"k", b"1", b"NX", b"XX"]),
        Reply::Error(_)
    ));
    assert!(matches!(
        client.call(&[b"EXPIRE", b"k", b"1", b"GT", b"LT"]),
        Reply::Error(_)
    ));
    assert_eq!(
        client.call(&[b"EXPIRE", b"k", b"1", b"bad\r\n+fake"]),
        Reply::Error(b"ERR Unsupported option bad  +fake".to_vec())
    );
    assert_eq!(
        client.call(&[b"PING", b"still connected"]),
        Reply::Bulk(Some(b"still connected".to_vec()))
    );
    assert_eq!(client.call(&[b"TTL", b"k"]), integer(-1));
}

#[test]
fn watch_same_value_write_aborts_exec_and_clears_watches() {
    let server = common::Server::start();
    let mut client = Client::new(&server);
    let mut writer = Client::new(&server);
    assert_eq!(writer.call(&[b"SET", b"k", b"v"]), ok());
    assert_eq!(client.call(&[b"WATCH", b"k", b"k"]), ok());
    assert_eq!(writer.call(&[b"SET", b"k", b"v"]), ok());
    assert_eq!(client.call(&[b"WATCH", b"k"]), ok());
    assert_eq!(client.call(&[b"MULTI"]), ok());
    assert_eq!(
        client.call(&[b"SET", b"result", b"should not run"]),
        queued()
    );
    assert_eq!(client.call(&[b"EXEC"]), Reply::Array(None));
    assert_eq!(client.call(&[b"GET", b"result"]), Reply::Bulk(None));
    assert_eq!(client.call(&[b"MULTI"]), ok());
    assert_eq!(client.call(&[b"SET", b"result", b"runs"]), queued());
    assert_eq!(client.call(&[b"EXEC"]), array(vec![ok()]));
}

#[test]
fn watch_missing_and_multiple_keys_tracks_create_delete_cycles() {
    let server = common::Server::start();
    let mut client = Client::new(&server);
    let mut writer = Client::new(&server);
    assert_eq!(client.call(&[b"WATCH", b"missing", b"other"]), ok());
    assert_eq!(writer.call(&[b"SET", b"missing", b"v"]), ok());
    assert_eq!(writer.call(&[b"DEL", b"missing"]), integer(1));
    assert_eq!(client.call(&[b"MULTI"]), ok());
    assert_eq!(client.call(&[b"EXEC"]), Reply::Array(None));
    assert_eq!(client.call(&[b"WATCH", b"missing", b"other"]), ok());
    assert_eq!(writer.call(&[b"SET", b"unrelated", b"v"]), ok());
    assert_eq!(client.call(&[b"MULTI"]), ok());
    assert_eq!(client.call(&[b"EXEC"]), array(vec![]));
}

#[test]
fn unwatch_discard_and_successful_exec_release_watches() {
    let server = common::Server::start();
    let mut client = Client::new(&server);
    let mut writer = Client::new(&server);
    for end in [b"UNWATCH".as_slice(), b"DISCARD", b"EXEC"] {
        assert_eq!(client.call(&[b"WATCH", b"k"]), ok());
        if end != b"UNWATCH" {
            assert_eq!(client.call(&[b"MULTI"]), ok());
        }
        if end == b"EXEC" {
            assert_eq!(client.call(&[end]), array(vec![]));
        } else {
            assert_eq!(client.call(&[end]), ok());
        }
        assert_eq!(writer.call(&[b"SET", b"k", b"v"]), ok());
        assert_eq!(client.call(&[b"MULTI"]), ok());
        assert_eq!(client.call(&[b"EXEC"]), array(vec![]));
    }
    assert_eq!(client.call(&[b"MULTI"]), ok());
    assert_eq!(
        client.call(&[b"WATCH", b"k"]),
        Reply::Error(b"ERR WATCH inside MULTI is not allowed".to_vec())
    );
    assert_eq!(client.call(&[b"UNWATCH"]), queued());
    assert_eq!(client.call(&[b"EXEC"]), array(vec![ok()]));
}

#[test]
fn natural_expiry_aborts_even_when_another_client_removes_expired_key() {
    let server = common::Server::start();
    let mut client = Client::new(&server);
    let mut observer = Client::new(&server);
    assert_eq!(client.call(&[b"SET", b"k", b"v", b"PX", b"150"]), ok());
    assert_eq!(client.call(&[b"WATCH", b"k"]), ok());
    std::thread::sleep(Duration::from_millis(210));
    assert_eq!(observer.call(&[b"GET", b"k"]), Reply::Bulk(None));
    assert_eq!(client.call(&[b"MULTI"]), ok());
    assert_eq!(client.call(&[b"EXEC"]), Reply::Array(None));
}

#[test]
fn failed_or_noop_writes_preserve_watch_but_real_changes_abort() {
    let server = common::Server::start();
    let mut watcher = Client::new(&server);
    let mut writer = Client::new(&server);
    assert_eq!(writer.call(&[b"RPUSH", b"list", b"a"]), integer(1));
    assert_eq!(watcher.call(&[b"WATCH", b"list"]), ok());
    assert!(matches!(writer.call(&[b"INCR", b"list"]), Reply::Error(_)));
    assert_eq!(writer.call(&[b"LPOP", b"list", b"0"]), array(vec![]));
    assert_eq!(
        writer.call(&[b"EXPIRE", b"list", b"100", b"XX"]),
        integer(0)
    );
    assert_eq!(writer.call(&[b"PERSIST", b"list"]), integer(0));
    assert_eq!(watcher.call(&[b"MULTI"]), ok());
    assert_eq!(watcher.call(&[b"EXEC"]), array(vec![]));
    assert_eq!(watcher.call(&[b"WATCH", b"list"]), ok());
    assert_eq!(writer.call(&[b"EXPIRE", b"list", b"100"]), integer(1));
    assert_eq!(watcher.call(&[b"MULTI"]), ok());
    assert_eq!(watcher.call(&[b"EXEC"]), Reply::Array(None));
}

#[test]
fn blocking_pop_only_invalidates_the_key_it_removes_from() {
    let server = common::Server::start();
    let mut first = Client::new(&server);
    let mut second = Client::new(&server);
    let mut writer = Client::new(&server);
    assert_eq!(writer.call(&[b"RPUSH", b"list", b"a"]), integer(1));
    assert_eq!(first.call(&[b"WATCH", b"list"]), ok());
    assert_eq!(second.call(&[b"WATCH", b"missing"]), ok());
    assert_eq!(
        writer.call(&[b"BLPOP", b"missing", b"list", b"0"]),
        array(vec![
            Reply::Bulk(Some(b"list".to_vec())),
            Reply::Bulk(Some(b"a".to_vec()))
        ])
    );
    assert_eq!(first.call(&[b"MULTI"]), ok());
    assert_eq!(first.call(&[b"EXEC"]), Reply::Array(None));
    assert_eq!(second.call(&[b"MULTI"]), ok());
    assert_eq!(second.call(&[b"EXEC"]), array(vec![]));
}

fn wait_for_bulk(client: &mut Client, key: &[u8], expected: &[u8]) {
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    loop {
        if client.call(&[b"GET", key]) == Reply::Bulk(Some(expected.to_vec())) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "replication timed out"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn replica_watch_tracks_same_value_writes_without_aborting_for_unrelated_keys() {
    let primary = common::Server::start();
    let mut writer = Client::new(&primary);
    assert_eq!(writer.call(&[b"SET", b"watched", b"v"]), ok());
    let port = primary.port.to_string();
    let replica = common::Server::start_with(&["--replicaof", "127.0.0.1", &port]);
    let mut watcher = Client::new(&replica);
    wait_for_bulk(&mut watcher, b"watched", b"v");
    assert_eq!(watcher.call(&[b"WATCH", b"watched"]), ok());
    assert_eq!(writer.call(&[b"SET", b"sentinel", b"1"]), ok());
    wait_for_bulk(&mut watcher, b"sentinel", b"1");
    assert_eq!(watcher.call(&[b"MULTI"]), ok());
    assert_eq!(watcher.call(&[b"EXEC"]), array(vec![]));

    assert_eq!(watcher.call(&[b"WATCH", b"watched"]), ok());
    assert_eq!(writer.call(&[b"SET", b"watched", b"v"]), ok());
    assert_eq!(writer.call(&[b"SET", b"sentinel", b"2"]), ok());
    wait_for_bulk(&mut watcher, b"sentinel", b"2");
    assert_eq!(watcher.call(&[b"MULTI"]), ok());
    assert_eq!(watcher.call(&[b"EXEC"]), Reply::Array(None));

    assert_eq!(writer.call(&[b"EXPIRE", b"watched", b"100"]), integer(1));
    assert_eq!(writer.call(&[b"SET", b"sentinel", b"3"]), ok());
    wait_for_bulk(&mut watcher, b"sentinel", b"3");
    assert_eq!(watcher.call(&[b"TTL", b"watched"]), integer(100));
    assert_eq!(writer.call(&[b"PERSIST", b"watched"]), integer(1));
    assert_eq!(writer.call(&[b"DEL", b"watched"]), integer(1));
    assert_eq!(writer.call(&[b"SET", b"sentinel", b"4"]), ok());
    wait_for_bulk(&mut watcher, b"sentinel", b"4");
    assert_eq!(watcher.call(&[b"EXISTS", b"watched"]), integer(0));
}

#[test]
fn keyspace_changes_survive_snapshot_restart() {
    let directory = common::temp_path("reru-keyspace", "");
    std::fs::create_dir_all(&directory).unwrap();
    let snapshot = directory.join("data.snap");
    let path = snapshot.to_str().unwrap();
    {
        let server = common::Server::start_with(&["--snapshot", path]);
        let mut client = Client::new(&server);
        assert_eq!(client.call(&[b"RPUSH", b"list", b"a"]), integer(1));
        assert_eq!(client.call(&[b"EXPIRE", b"list", b"100"]), integer(1));
        assert_eq!(
            client.call(&[b"SET", b"persistent", b"v", b"PX", b"10000"]),
            ok()
        );
        assert_eq!(client.call(&[b"PERSIST", b"persistent"]), integer(1));
        assert_eq!(client.call(&[b"SET", b"deleted", b"v"]), ok());
        assert_eq!(client.call(&[b"DEL", b"deleted"]), integer(1));
    }
    {
        let server = common::Server::start_with(&["--snapshot", path]);
        let mut client = Client::new(&server);
        assert_eq!(client.call(&[b"LLEN", b"list"]), integer(1));
        let Reply::Integer(ttl) = client.call(&[b"TTL", b"list"]) else {
            panic!("expected TTL")
        };
        assert!((95..=100).contains(&ttl));
        assert_eq!(client.call(&[b"TTL", b"persistent"]), integer(-1));
        assert_eq!(
            client.call(&[b"GET", b"persistent"]),
            Reply::Bulk(Some(b"v".to_vec()))
        );
        assert_eq!(client.call(&[b"EXISTS", b"deleted"]), integer(0));
    }
    std::fs::remove_file(snapshot).unwrap();
    std::fs::remove_file(directory.join("data.bak")).unwrap();
    std::fs::remove_dir(directory).unwrap();
}

#[test]
fn exec_writes_invalidate_other_watchers_and_queue_error_clears_watches() {
    let server = common::Server::start();
    let mut watcher = Client::new(&server);
    let mut writer = Client::new(&server);
    assert_eq!(watcher.call(&[b"WATCH", b"k"]), ok());
    assert_eq!(writer.call(&[b"MULTI"]), ok());
    assert_eq!(writer.call(&[b"SET", b"k", b"v"]), queued());
    assert_eq!(writer.call(&[b"EXPIRE", b"k", b"100"]), queued());
    assert_eq!(writer.call(&[b"EXEC"]), array(vec![ok(), integer(1)]));
    assert_eq!(watcher.call(&[b"MULTI"]), ok());
    assert_eq!(watcher.call(&[b"EXEC"]), Reply::Array(None));

    assert_eq!(watcher.call(&[b"WATCH", b"k"]), ok());
    assert_eq!(watcher.call(&[b"MULTI"]), ok());
    assert!(matches!(watcher.call(&[b"DEL"]), Reply::Error(_)));
    assert!(matches!(watcher.call(&[b"EXEC"]), Reply::Error(_)));
    assert_eq!(writer.call(&[b"DEL", b"k"]), integer(1));
    assert_eq!(watcher.call(&[b"MULTI"]), ok());
    assert_eq!(watcher.call(&[b"EXEC"]), array(vec![]));
}
