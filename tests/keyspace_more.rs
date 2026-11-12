// SPDX-License-Identifier: AGPL-3.0-only
#[path = "common/client.rs"]
#[allow(dead_code)]
mod client;
#[allow(dead_code)]
mod common;

use client::{integer, ok, Client, Reply};

fn bulk(value: &[u8]) -> Reply {
    Reply::Bulk(Some(value.to_vec()))
}

#[test]
fn rename_copy_and_renamenx_preserve_source_expiry() {
    let server = common::Server::start();
    let mut client = Client::new(&server);
    assert_eq!(
        client.call(&[b"SET", b"source", b"data", b"PX", b"6000"]),
        ok()
    );
    assert_eq!(client.call(&[b"COPY", b"source", b"copy"]), integer(1));
    assert!(matches!(client.call(&[b"PTTL", b"copy"]), Reply::Integer(value) if value > 0));
    assert_eq!(client.call(&[b"SET", b"target", b"old"]), ok());
    assert_eq!(client.call(&[b"RENAME", b"source", b"target"]), ok());
    assert_eq!(client.call(&[b"GET", b"source"]), Reply::Bulk(None));
    assert_eq!(client.call(&[b"GET", b"target"]), bulk(b"data"));
    assert!(matches!(client.call(&[b"PTTL", b"target"]), Reply::Integer(value) if value > 0));
    assert_eq!(client.call(&[b"COPY", b"target", b"copy"]), integer(0));
    assert_eq!(
        client.call(&[b"COPY", b"target", b"copy", b"REPLACE"]),
        integer(1)
    );
    assert_eq!(client.call(&[b"RENAMENX", b"target", b"copy"]), integer(0));
    assert_eq!(client.call(&[b"RENAMENX", b"target", b"moved"]), integer(1));
    assert_eq!(client.call(&[b"RENAME", b"moved", b"moved"]), ok());
    assert_eq!(client.call(&[b"RENAMENX", b"moved", b"moved"]), integer(0));
    assert!(matches!(
        client.call(&[b"RENAME", b"absent", b"x"]),
        Reply::Error(_)
    ));
}

#[test]
fn unlink_touch_and_dbsize_work_across_data_types() {
    let server = common::Server::start();
    let mut client = Client::new(&server);
    assert_eq!(client.call(&[b"SET", b"string", b"v"]), ok());
    assert_eq!(client.call(&[b"SADD", b"set", b"member"]), integer(1));
    assert_eq!(client.call(&[b"DBSIZE"]), integer(2));
    assert_eq!(
        client.call(&[b"TOUCH", b"string", b"set", b"absent"]),
        integer(2)
    );
    assert_eq!(
        client.call(&[b"UNLINK", b"string", b"set", b"absent"]),
        integer(2)
    );
    assert_eq!(client.call(&[b"DBSIZE"]), integer(0));
    assert_eq!(client.call(&[b"TOUCH", b"string"]), integer(0));
}

#[test]
fn copy_oom_rolls_back_and_preserves_watch() {
    let server = common::Server::start_with(&["--maxmemory", "220"]);
    let mut client = Client::new(&server);
    let mut watcher = Client::new(&server);
    let value = vec![b'a'; 100];
    assert_eq!(client.call(&[b"SET", b"a", &value]), ok());
    assert_eq!(watcher.call(&[b"WATCH", b"b"]), ok());
    assert!(
        matches!(client.call(&[b"COPY", b"a", b"b"]), Reply::Error(message) if message.starts_with(b"OOM "))
    );
    assert_eq!(client.call(&[b"GET", b"b"]), Reply::Bulk(None));
    assert_eq!(watcher.call(&[b"MULTI"]), ok());
    assert_eq!(watcher.call(&[b"GET", b"a"]), client::queued());
    assert_eq!(watcher.call(&[b"EXEC"]), client::array(vec![bulk(&value)]));
}

#[test]
fn touch_changes_lru_victim() {
    let server =
        common::Server::start_with(&["--maxmemory", "330", "--maxmemory-policy", "allkeys-lru"]);
    let mut client = Client::new(&server);
    let value = vec![b'v'; 100];
    assert_eq!(client.call(&[b"SET", b"a", &value]), ok());
    assert_eq!(client.call(&[b"SET", b"b", &value]), ok());
    assert_eq!(client.call(&[b"TOUCH", b"a"]), integer(1));
    assert_eq!(client.call(&[b"SET", b"c", &value]), ok());
    assert_eq!(client.call(&[b"GET", b"b"]), Reply::Bulk(None));
    assert_eq!(client.call(&[b"GET", b"a"]), bulk(&value));
}

#[test]
fn rename_to_a_blocked_list_key_wakes_the_waiter() {
    let server = common::Server::start();
    let mut client = Client::new(&server);
    let mut blocked = Client::new(&server);
    let waiter = std::thread::spawn(move || blocked.call(&[b"BLPOP", b"queue", b"2"]));
    std::thread::sleep(std::time::Duration::from_millis(80));
    assert_eq!(client.call(&[b"RPUSH", b"temporary", b"item"]), integer(1));
    assert_eq!(client.call(&[b"RENAME", b"temporary", b"queue"]), ok());
    assert_eq!(
        waiter.join().unwrap(),
        client::array(vec![bulk(b"queue"), bulk(b"item")])
    );
}

#[test]
fn brpop_reads_the_right_end_and_wakes_for_a_new_list() {
    let server = common::Server::start();
    let mut writer = Client::new(&server);
    assert_eq!(
        writer.call(&[b"RPUSH", b"existing", b"left", b"right"]),
        integer(2)
    );
    assert_eq!(
        writer.call(&[b"BRPOP", b"missing", b"existing", b"1"]),
        client::array(vec![bulk(b"existing"), bulk(b"right")])
    );
    assert_eq!(
        writer.call(&[b"LRANGE", b"existing", b"0", b"-1"]),
        client::array(vec![bulk(b"left")])
    );
    let mut blocked = Client::new(&server);
    let waiter = std::thread::spawn(move || blocked.call(&[b"BRPOP", b"new", b"2"]));
    std::thread::sleep(std::time::Duration::from_millis(80));
    assert_eq!(writer.call(&[b"LPUSH", b"new", b"a", b"b"]), integer(2));
    assert_eq!(
        waiter.join().unwrap(),
        client::array(vec![bulk(b"new"), bulk(b"a")])
    );
    assert_eq!(
        writer.call(&[b"LRANGE", b"new", b"0", b"-1"]),
        client::array(vec![bulk(b"b")])
    );
}
