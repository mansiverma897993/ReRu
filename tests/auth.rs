// SPDX-License-Identifier: AGPL-3.0-only
#[path = "common/client.rs"]
#[allow(dead_code)]
mod client;
#[allow(dead_code)]
mod common;

use client::{array, ok, queued, Client, Reply};
use common::Server;
use std::time::{Duration, Instant};

fn noauth() -> Reply {
    Reply::Error(b"NOAUTH Authentication required.".to_vec())
}
fn wrongpass() -> Reply {
    Reply::Error(b"WRONGPASS invalid username-password pair or user is disabled.".to_vec())
}
fn pong() -> Reply {
    Reply::Simple(b"PONG".to_vec())
}
fn bulk(bytes: &[u8]) -> Reply {
    Reply::Bulk(Some(bytes.to_vec()))
}
fn path() -> std::path::PathBuf {
    common::temp_path("reru-auth", "")
}

#[test]
fn unauthenticated_clients_cannot_read_write_subscribe_or_bypass_through_replication() {
    let server = Server::start_with(&["--requirepass", "test-password"]);
    let mut c = Client::new(&server);
    for command in [
        vec![b"PING".as_slice()],
        vec![b"SET", b"k", b"v"],
        vec![b"GET", b"k"],
        vec![b"MULTI"],
        vec![b"EXEC"],
        vec![b"WATCH", b"k"],
        vec![b"INFO"],
        vec![b"CONFIG", b"GET", b"*"],
        vec![b"SUBSCRIBE", b"channel"],
        vec![b"REPLSYNC"],
        vec![b"ACL", b"WHOAMI"],
        vec![b"BGREWRITEAOF"],
    ] {
        assert_eq!(c.call(&command), noauth(), "{command:?}");
    }
    assert_eq!(c.call(&[b"AUTH", b"bad"]), wrongpass());
    assert_eq!(c.call(&[b"PING"]), noauth());
    assert!(matches!(c.call(&[b"AUTH"]), Reply::Error(_)));
    assert_eq!(
        c.call(&[b"AUTH", b"unknown", b"test-password"]),
        wrongpass()
    );
    assert_eq!(c.call(&[b"AUTH", b"default", b"test-password"]), ok());
    assert_eq!(c.call(&[b"ACL", b"WHOAMI"]), bulk(b"default"));
    assert_eq!(c.call(&[b"SET", b"k", b"v"]), ok());
    let mut other = Client::new(&server);
    assert_eq!(other.call(&[b"GET", b"k"]), noauth());
    assert_eq!(other.call(&[b"QUIT"]), ok());
}

#[test]
fn authentication_in_transactions_and_failed_reauth_preserve_the_session() {
    let server = Server::start_with(&["--requirepass", "test-password"]);
    let mut c = Client::new(&server);
    assert_eq!(c.call(&[b"AUTH", b"test-password"]), ok());
    assert_eq!(c.call(&[b"AUTH", b"bad"]), wrongpass());
    assert_eq!(c.call(&[b"PING"]), pong());
    c.call(&[b"MULTI"]);
    assert_eq!(c.call(&[b"AUTH", b"bad"]), queued());
    assert_eq!(c.call(&[b"ACL", b"WHOAMI"]), queued());
    assert_eq!(c.call(&[b"SET", b"key", b"value"]), queued());
    assert_eq!(
        c.call(&[b"EXEC"]),
        array(vec![wrongpass(), bulk(b"default"), ok()])
    );
    c.call(&[b"MULTI"]);
    assert!(matches!(c.call(&[b"AUTH"]), Reply::Error(_)));
    assert!(matches!(c.call(&[b"EXEC"]), Reply::Error(e) if e.starts_with(b"EXECABORT")));
    c.call(&[b"SUBSCRIBE", b"channel"]);
    assert!(
        matches!(c.call(&[b"AUTH", b"test-password"]), Reply::Error(e) if e.starts_with(b"ERR Can't execute"))
    );
    c.call(&[b"UNSUBSCRIBE"]);
    assert_eq!(c.call(&[b"PING"]), pong());
}

#[test]
fn default_nopass_mode_and_username_case_match_redis() {
    let server = Server::start();
    let mut c = Client::new(&server);
    assert_eq!(c.call(&[b"PING"]), pong());
    assert!(
        matches!(c.call(&[b"AUTH", b"unused"]), Reply::Error(e) if e.starts_with(b"ERR AUTH <password> called without any password configured"))
    );
    assert_eq!(c.call(&[b"AUTH", b"default", b"anything"]), ok());
    assert_eq!(c.call(&[b"AUTH", b"Default", b"anything"]), wrongpass());
    assert_eq!(c.call(&[b"ACL", b"WHOAMI"]), bulk(b"default"));
    assert!(matches!(
        c.call(&[b"ACL", b"SETUSER", b"user"]),
        Reply::Error(_)
    ));
}

#[test]
fn password_files_accept_binary_content_trim_one_line_ending_and_bound_size() {
    let path = path();
    std::fs::write(&path, b"binary\0\xff \r\n").unwrap();
    {
        let server = Server::start_with(&["--requirepass-file", path.to_str().unwrap()]);
        let mut c = Client::new(&server);
        assert_eq!(c.call(&[b"AUTH", b"binary\0\xff"]), wrongpass());
        assert_eq!(c.call(&[b"AUTH", b"binary\0\xff "]), ok());
        assert_eq!(c.call(&[b"PING"]), pong());
    }
    std::fs::write(&path, [b'x'; 4097]).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_reru"))
        .args(["--requirepass-file", path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("password file exceeds 4096 bytes"));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn persisted_data_replays_without_storing_credentials_and_new_clients_must_authenticate() {
    let path = path();
    let filename = path.to_str().unwrap();
    let secret = b"credential-not-for-persistence";
    for iteration in 0..2 {
        let server = Server::start_with(&[
            "--requirepass",
            std::str::from_utf8(secret).unwrap(),
            "--appendonly",
            filename,
        ]);
        let mut c = Client::new(&server);
        assert_eq!(c.call(&[b"GET", b"key"]), noauth());
        assert_eq!(c.call(&[b"AUTH", secret]), ok());
        if iteration == 0 {
            assert_eq!(c.call(&[b"SET", b"key", b"value"]), ok());
        }
        assert_eq!(c.call(&[b"GET", b"key"]), bulk(b"value"));
        assert_eq!(c.call(&[b"CONFIG", b"GET", b"requirepass"]), array(vec![]));
    }
    assert!(!std::fs::read(&path)
        .unwrap()
        .windows(secret.len())
        .any(|w| w == secret));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn replicas_authenticate_to_primary_and_enforce_their_own_password() {
    let path = path();
    std::fs::write(&path, b"primary-password\n").unwrap();
    {
        let primary = Server::start_with(&["--requirepass-file", path.to_str().unwrap()]);
        let mut writer = Client::new(&primary);
        writer.call(&[b"AUTH", b"primary-password"]);
        writer.call(&[b"SET", b"key", b"initial"]);
        let replica = Server::start_with(&[
            "--replicaof",
            "127.0.0.1",
            &primary.port.to_string(),
            "--masterauth-file",
            path.to_str().unwrap(),
            "--requirepass",
            "replica-password",
        ]);
        let wrong = Server::start_with(&[
            "--replicaof",
            "127.0.0.1",
            &primary.port.to_string(),
            "--masterauth",
            "wrong",
        ]);
        let mut reader = Client::new(&replica);
        assert_eq!(reader.call(&[b"GET", b"key"]), noauth());
        assert_eq!(reader.call(&[b"AUTH", b"replica-password"]), ok());
        for value in [b"initial".as_slice(), b"updated"] {
            writer.call(&[b"SET", b"key", value]);
            let deadline = Instant::now() + Duration::from_secs(5);
            while reader.call(&[b"GET", b"key"]) != bulk(value) {
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        assert!(
            matches!(reader.call(&[b"SET", b"key", b"v"]), Reply::Error(e) if e.starts_with(b"READONLY"))
        );
        let mut wrong_reader = Client::new(&wrong);
        assert_eq!(wrong_reader.call(&[b"GET", b"key"]), Reply::Bulk(None));
    }
    std::fs::remove_file(path).unwrap();
}
