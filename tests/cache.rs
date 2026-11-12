// SPDX-License-Identifier: AGPL-3.0-only
#[path = "common/client.rs"]
mod client;
mod common;
use client::{array, integer, ok, queued, Client, Reply};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn bulk(value: &[u8]) -> Reply {
    Reply::Bulk(Some(value.to_vec()))
}

#[test]
fn floating_increment_and_substr_preserve_redis_string_behavior() {
    let server = common::Server::start();
    let mut c = Client::new(&server);
    assert_eq!(c.call(&[b"INCRBYFLOAT", b"amount", b"10.5"]), bulk(b"10.5"));
    assert_eq!(c.call(&[b"INCRBYFLOAT", b"amount", b"0.1"]), bulk(b"10.6"));
    assert_eq!(c.call(&[b"GET", b"amount"]), bulk(b"10.6"));
    assert_eq!(c.call(&[b"SUBSTR", b"amount", b"-3", b"-1"]), bulk(b"0.6"));
    assert_eq!(c.call(&[b"SUBSTR", b"missing", b"0", b"-1"]), bulk(b""));
    assert_eq!(c.call(&[b"SET", b"amount", b"5e3", b"EX", b"60"]), ok());
    assert_eq!(c.call(&[b"INCRBYFLOAT", b"amount", b"2e2"]), bulk(b"5200"));
    assert!(matches!(c.call(&[b"TTL", b"amount"]), Reply::Integer(ttl) if ttl > 0));
    assert_eq!(c.call(&[b"SET", b"amount", b"1e308"]), ok());
    assert!(matches!(
        c.call(&[b"INCRBYFLOAT", b"amount", b"1e308"]),
        Reply::Error(_)
    ));
    assert_eq!(c.call(&[b"GET", b"amount"]), bulk(b"1e308"));
    assert!(matches!(
        c.call(&[b"INCRBYFLOAT", b"amount", b"nan"]),
        Reply::Error(_)
    ));
    assert_eq!(c.call(&[b"RPUSH", b"list", b"a"]), integer(1));
    assert!(matches!(
        c.call(&[b"INCRBYFLOAT", b"list", b"1"]),
        Reply::Error(_)
    ));
    assert!(matches!(
        c.call(&[b"SUBSTR", b"list", b"0", b"1"]),
        Reply::Error(_)
    ));
    assert_eq!(c.call(&[b"MULTI"]), ok());
    assert_eq!(c.call(&[b"INCRBYFLOAT", b"fresh", b"0.5"]), queued());
    assert_eq!(c.call(&[b"SUBSTR", b"fresh", b"0", b"-1"]), queued());
    assert_eq!(c.call(&[b"EXEC"]), array(vec![bulk(b"0.5"), bulk(b"0.5")]));
}

#[test]
fn time_returns_current_epoch_seconds_and_microseconds() {
    let server = common::Server::start();
    let mut c = Client::new(&server);
    let before = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let reply = c.call(&[b"TIME"]);
    let after = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let Reply::Array(Some(parts)) = reply else {
        panic!("TIME must return two bulk strings");
    };
    assert_eq!(parts.len(), 2);
    let Reply::Bulk(Some(seconds)) = &parts[0] else {
        panic!("seconds must be a bulk string")
    };
    let Reply::Bulk(Some(micros)) = &parts[1] else {
        panic!("microseconds must be a bulk string")
    };
    let seconds: u64 = std::str::from_utf8(seconds).unwrap().parse().unwrap();
    let micros: u32 = std::str::from_utf8(micros).unwrap().parse().unwrap();
    assert!((before..=after).contains(&seconds));
    assert!(micros < 1_000_000);
    assert!(matches!(c.call(&[b"TIME", b"extra"]), Reply::Error(_)));
}

#[test]
fn legacy_set_commands_preserve_expected_conditional_and_expiry_behavior() {
    let server = common::Server::start();
    let mut c = Client::new(&server);
    assert_eq!(c.call(&[b"SETNX", b"k", b"first"]), integer(1));
    assert_eq!(c.call(&[b"SETNX", b"k", b"second"]), integer(0));
    assert_eq!(c.call(&[b"GETSET", b"k", b"third"]), bulk(b"first"));
    assert_eq!(c.call(&[b"GET", b"k"]), bulk(b"third"));
    assert_eq!(c.call(&[b"SETEX", b"k", b"60", b"ex"]), ok());
    assert!(matches!(c.call(&[b"TTL", b"k"]), Reply::Integer(ttl) if ttl > 0));
    assert_eq!(c.call(&[b"PSETEX", b"k", b"60000", b"px"]), ok());
    assert!(matches!(c.call(&[b"PTTL", b"k"]), Reply::Integer(ttl) if ttl > 0));
    assert!(matches!(
        c.call(&[b"SETEX", b"k", b"0", b"bad"]),
        Reply::Error(_)
    ));
    assert_eq!(c.call(&[b"GET", b"k"]), bulk(b"px"));
}

#[test]
fn integer_and_range_commands_handle_overflow_binary_padding_and_wrong_types() {
    let server = common::Server::start();
    let mut c = Client::new(&server);
    assert_eq!(c.call(&[b"INCRBY", b"counter", b"5"]), integer(5));
    assert_eq!(c.call(&[b"DECR", b"counter"]), integer(4));
    assert_eq!(c.call(&[b"DECRBY", b"counter", b"-2"]), integer(6));
    assert_eq!(c.call(&[b"SET", b"max", b"9223372036854775807"]), ok());
    assert!(matches!(
        c.call(&[b"INCRBY", b"max", b"1"]),
        Reply::Error(_)
    ));
    assert_eq!(c.call(&[b"GET", b"max"]), bulk(b"9223372036854775807"));
    assert_eq!(c.call(&[b"GETRANGE", b"missing", b"0", b"-1"]), bulk(b""));
    assert_eq!(c.call(&[b"SETRANGE", b"binary", b"2", b"a\0b"]), integer(5));
    assert_eq!(c.call(&[b"GET", b"binary"]), bulk(b"\0\0a\0b"));
    assert_eq!(
        c.call(&[b"GETRANGE", b"binary", b"-3", b"-1"]),
        bulk(b"a\0b")
    );
    assert_eq!(c.call(&[b"SETRANGE", b"binary", b"3", b""]), integer(5));
    assert_eq!(c.call(&[b"RPUSH", b"list", b"v"]), integer(1));
    assert!(matches!(
        c.call(&[b"GETRANGE", b"list", b"0", b"-1"]),
        Reply::Error(_)
    ));
    assert!(matches!(
        c.call(&[b"SETRANGE", b"list", b"0", b"x"]),
        Reply::Error(_)
    ));
}

#[test]
fn set_conditions_get_and_keepttl_support_cache_locks() {
    let server = common::Server::start();
    let mut c = Client::new(&server);
    assert_eq!(
        c.call(&[b"SET", b"lock", b"one", b"NX", b"EX", b"60"]),
        ok()
    );
    assert_eq!(c.call(&[b"SET", b"lock", b"two", b"NX"]), Reply::Bulk(None));
    assert_eq!(
        c.call(&[b"SET", b"missing", b"x", b"XX"]),
        Reply::Bulk(None)
    );
    assert_eq!(
        c.call(&[b"SET", b"lock", b"two", b"NX", b"GET"]),
        bulk(b"one")
    );
    assert_eq!(c.call(&[b"GET", b"lock"]), bulk(b"one"));
    assert_eq!(
        c.call(&[b"SET", b"lock", b"three", b"XX", b"GET", b"KEEPTTL"]),
        bulk(b"one")
    );
    assert_eq!(c.call(&[b"GET", b"lock"]), bulk(b"three"));
    let Reply::Integer(ttl) = c.call(&[b"TTL", b"lock"]) else {
        panic!("expected TTL")
    };
    assert!((55..=60).contains(&ttl));
    assert_eq!(c.call(&[b"SET", b"lock", b"final"]), ok());
    assert_eq!(c.call(&[b"TTL", b"lock"]), integer(-1));
    assert_eq!(
        c.call(&[b"SET", b"empty", b"first", b"GET"]),
        Reply::Bulk(None)
    );
    assert_eq!(c.call(&[b"GET", b"empty"]), bulk(b"first"));
}

#[test]
fn set_rejects_bad_options_and_get_wrong_type_without_overwriting() {
    let server = common::Server::start();
    let mut c = Client::new(&server);
    assert_eq!(c.call(&[b"RPUSH", b"list", b"x"]), integer(1));
    assert!(matches!(
        c.call(&[b"SET", b"list", b"x", b"GET"]),
        Reply::Error(_)
    ));
    assert_eq!(c.call(&[b"TYPE", b"list"]), Reply::Simple(b"list".to_vec()));
    for args in [
        &[b"SET".as_slice(), b"missing", b"v", b"NX", b"XX"][..],
        &[b"SET".as_slice(), b"missing", b"v", b"PX", b"0"][..],
        &[
            b"SET".as_slice(),
            b"missing",
            b"v",
            b"PX",
            b"10",
            b"EX",
            b"1",
        ][..],
        &[
            b"SET".as_slice(),
            b"missing",
            b"v",
            b"KEEPTTL",
            b"PX",
            b"10",
        ][..],
        &[b"SET".as_slice(), b"missing", b"v", b"GET", b"GET"][..],
    ] {
        assert!(matches!(c.call(args), Reply::Error(_)));
    }
    assert_eq!(c.call(&[b"EXISTS", b"missing"]), integer(0));
}

#[test]
fn mset_and_msetnx_are_atomic_and_mget_handles_non_strings_as_nil() {
    let server = common::Server::start();
    let mut c = Client::new(&server);
    assert_eq!(
        c.call(&[b"MSET", b"a\0", b"1", b"b", b"2", b"a\0", b"last"]),
        ok()
    );
    assert_eq!(
        c.call(&[b"MGET", b"a\0", b"b", b"missing", b"a\0"]),
        array(vec![
            bulk(b"last"),
            bulk(b"2"),
            Reply::Bulk(None),
            bulk(b"last")
        ])
    );
    assert_eq!(
        c.call(&[b"MSETNX", b"new", b"x", b"b", b"overwritten"]),
        integer(0)
    );
    assert_eq!(c.call(&[b"EXISTS", b"new"]), integer(0));
    assert_eq!(c.call(&[b"GET", b"b"]), bulk(b"2"));
    assert_eq!(
        c.call(&[b"MSETNX", b"new", b"x", b"new", b"last"]),
        integer(1)
    );
    assert_eq!(c.call(&[b"GET", b"new"]), bulk(b"last"));
    assert_eq!(c.call(&[b"RPUSH", b"list", b"x"]), integer(1));
    assert_eq!(
        c.call(&[b"MGET", b"list", b"new"]),
        array(vec![Reply::Bulk(None), bulk(b"last")])
    );
    assert!(matches!(
        c.call(&[b"MSET", b"bad", b"value", b"dangling"]),
        Reply::Error(_)
    ));
    assert_eq!(c.call(&[b"EXISTS", b"bad"]), integer(0));
}

#[test]
fn getex_getdel_append_and_strlen_preserve_expected_expiry() {
    let server = common::Server::start();
    let mut c = Client::new(&server);
    assert_eq!(
        c.call(&[b"GETEX", b"missing", b"PX", b"100"]),
        Reply::Bulk(None)
    );
    assert_eq!(c.call(&[b"GETDEL", b"missing"]), Reply::Bulk(None));
    assert_eq!(c.call(&[b"STRLEN", b"missing"]), integer(0));
    assert_eq!(c.call(&[b"APPEND", b"k", b"a\0"]), integer(2));
    assert_eq!(c.call(&[b"PEXPIRE", b"k", b"10000"]), integer(1));
    assert_eq!(c.call(&[b"APPEND", b"k", b"b"]), integer(3));
    assert_eq!(c.call(&[b"STRLEN", b"k"]), integer(3));
    assert_eq!(c.call(&[b"GETEX", b"k", b"PERSIST"]), bulk(b"a\0b"));
    assert_eq!(c.call(&[b"TTL", b"k"]), integer(-1));
    assert_eq!(c.call(&[b"GETEX", b"k", b"EX", b"10"]), bulk(b"a\0b"));
    assert_eq!(c.call(&[b"TTL", b"k"]), integer(10));
    assert_eq!(c.call(&[b"GETDEL", b"k"]), bulk(b"a\0b"));
    assert_eq!(c.call(&[b"EXISTS", b"k"]), integer(0));
    assert_eq!(c.call(&[b"SET", b"past", b"gone", b"PXAT", b"1"]), ok());
    assert_eq!(c.call(&[b"EXISTS", b"past"]), integer(0));
}

#[test]
fn cache_commands_participate_in_watch_transactions_and_replication() {
    let primary = common::Server::start();
    let mut watcher = Client::new(&primary);
    let mut writer = Client::new(&primary);
    assert_eq!(writer.call(&[b"SET", b"key", b"old"]), ok());
    assert_eq!(watcher.call(&[b"WATCH", b"key"]), ok());
    assert_eq!(
        writer.call(&[b"SET", b"key", b"new", b"NX"]),
        Reply::Bulk(None)
    );
    assert_eq!(
        writer.call(&[b"MSETNX", b"key", b"newer", b"missing", b"v"]),
        integer(0)
    );
    assert_eq!(watcher.call(&[b"MULTI"]), ok());
    assert_eq!(watcher.call(&[b"EXEC"]), array(vec![]));
    assert_eq!(watcher.call(&[b"WATCH", b"key"]), ok());
    assert_eq!(
        writer.call(&[b"MSET", b"key", b"new", b"other", b"v"]),
        ok()
    );
    assert_eq!(watcher.call(&[b"MULTI"]), ok());
    assert_eq!(watcher.call(&[b"EXEC"]), Reply::Array(None));
    assert_eq!(writer.call(&[b"MULTI"]), ok());
    assert_eq!(writer.call(&[b"GETDEL", b"key"]), queued());
    assert_eq!(writer.call(&[b"APPEND", b"other", b"!"]), queued());
    assert_eq!(
        writer.call(&[b"EXEC"]),
        array(vec![bulk(b"new"), integer(2)])
    );
    let port = primary.port.to_string();
    let replica = common::Server::start_with(&["--replicaof", "127.0.0.1", &port]);
    let mut reader = Client::new(&replica);
    let until = Instant::now() + Duration::from_secs(3);
    loop {
        if reader.call(&[b"GET", b"other"]) == bulk(b"v!") {
            break;
        }
        assert!(Instant::now() < until, "replica did not synchronize");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        reader.call(&[b"MGET", b"key", b"other"]),
        array(vec![Reply::Bulk(None), bulk(b"v!")])
    );
}
