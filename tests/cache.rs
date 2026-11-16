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
        &[b"SET".as_slice(), b"missing", b"v", b"GET", b"FOO"][..],
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

#[test]
fn lcs_returns_subsequence_length_and_indexes() {
    let server = common::Server::start();
    let mut c = Client::new(&server);
    assert_eq!(c.call(&[b"SET", b"first", b"ohmytext"]), ok());
    assert_eq!(c.call(&[b"SET", b"second", b"mynewtext"]), ok());
    assert_eq!(c.call(&[b"LCS", b"first", b"second"]), bulk(b"mytext"));
    assert_eq!(c.call(&[b"LCS", b"first", b"second", b"LEN"]), integer(6));
    assert_eq!(
        c.call(&[b"LCS", b"first", b"second", b"IDX"]),
        array(vec![
            bulk(b"matches"),
            array(vec![
                array(vec![
                    array(vec![integer(4), integer(7)]),
                    array(vec![integer(5), integer(8)]),
                ]),
                array(vec![
                    array(vec![integer(2), integer(3)]),
                    array(vec![integer(0), integer(1)]),
                ]),
            ]),
            bulk(b"len"),
            integer(6),
        ])
    );
    assert_eq!(
        c.call(&[b"LCS", b"first", b"second", b"IDX", b"WITHMATCHLEN"]),
        array(vec![
            bulk(b"matches"),
            array(vec![
                array(vec![
                    array(vec![integer(4), integer(7)]),
                    array(vec![integer(5), integer(8)]),
                    integer(4),
                ]),
                array(vec![
                    array(vec![integer(2), integer(3)]),
                    array(vec![integer(0), integer(1)]),
                    integer(2),
                ]),
            ]),
            bulk(b"len"),
            integer(6),
        ])
    );
    assert_eq!(
        c.call(&[b"LCS", b"first", b"second", b"IDX", b"MINMATCHLEN", b"3"]),
        array(vec![
            bulk(b"matches"),
            array(vec![array(vec![
                array(vec![integer(4), integer(7)]),
                array(vec![integer(5), integer(8)]),
            ])]),
            bulk(b"len"),
            integer(6),
        ])
    );
    assert_eq!(
        c.call(&[
            b"LCS",
            b"first",
            b"second",
            b"IDX",
            b"MINMATCHLEN",
            b"3",
            b"WITHMATCHLEN",
        ]),
        array(vec![
            bulk(b"matches"),
            array(vec![array(vec![
                array(vec![integer(4), integer(7)]),
                array(vec![integer(5), integer(8)]),
                integer(4),
            ])]),
            bulk(b"len"),
            integer(6),
        ])
    );
}

#[test]
fn lcs_handles_empty_missing_binary_and_tied_keys() {
    let server = common::Server::start();
    let mut c = Client::new(&server);
    assert_eq!(c.call(&[b"LCS", b"missing-one", b"missing-two"]), bulk(b""));
    assert_eq!(
        c.call(&[b"LCS", b"missing-one", b"missing-two", b"LEN"]),
        integer(0)
    );
    assert_eq!(
        c.call(&[b"LCS", b"missing-one", b"missing-two", b"IDX"]),
        array(vec![
            bulk(b"matches"),
            array(vec![]),
            bulk(b"len"),
            integer(0)
        ])
    );
    assert_eq!(c.call(&[b"SET", b"present", b"text"]), ok());
    assert_eq!(c.call(&[b"LCS", b"present", b"missing"]), bulk(b""));
    assert_eq!(c.call(&[b"SET", b"empty", b""]), ok());
    assert_eq!(c.call(&[b"LCS", b"empty", b"present"]), bulk(b""));

    assert_eq!(c.call(&[b"SET", b"bin-one", b"\x00\xffA\x00B"]), ok());
    assert_eq!(c.call(&[b"SET", b"bin-two", b"\x00\xffA\x00C"]), ok());
    assert_eq!(
        c.call(&[b"LCS", b"bin-one", b"bin-two"]),
        bulk(b"\x00\xffA\x00")
    );
    assert_eq!(
        c.call(&[b"LCS", b"bin-one", b"bin-two", b"IDX"]),
        array(vec![
            bulk(b"matches"),
            array(vec![array(vec![
                array(vec![integer(0), integer(3)]),
                array(vec![integer(0), integer(3)]),
            ])]),
            bulk(b"len"),
            integer(4),
        ])
    );

    assert_eq!(c.call(&[b"SET", b"tied-one", b"abc"]), ok());
    assert_eq!(c.call(&[b"SET", b"tied-two", b"bac"]), ok());
    assert_eq!(c.call(&[b"LCS", b"tied-one", b"tied-two"]), bulk(b"bc"));
    assert_eq!(
        c.call(&[b"LCS", b"tied-one", b"tied-two", b"IDX"]),
        array(vec![
            bulk(b"matches"),
            array(vec![
                array(vec![
                    array(vec![integer(2), integer(2)]),
                    array(vec![integer(2), integer(2)]),
                ]),
                array(vec![
                    array(vec![integer(1), integer(1)]),
                    array(vec![integer(0), integer(0)]),
                ]),
            ]),
            bulk(b"len"),
            integer(2),
        ])
    );
}

#[test]
fn lcs_rejects_bad_options_and_wrong_types() {
    let server = common::Server::start();
    let mut c = Client::new(&server);
    assert_eq!(c.call(&[b"SET", b"lcs-a", b"abc"]), ok());
    assert_eq!(c.call(&[b"SET", b"lcs-b", b"abc"]), ok());
    assert_eq!(c.call(&[b"RPUSH", b"lcs-list", b"x"]), integer(1));
    assert_eq!(
        c.call(&[b"LCS", b"lcs-a", b"lcs-b", b"LEN", b"IDX"]),
        Reply::Error(b"ERR If you want both the length and indexes, please just use IDX.".to_vec())
    );
    assert_eq!(
        c.call(&[b"LCS", b"lcs-a", b"lcs-b", b"FOO"]),
        Reply::Error(b"ERR syntax error".to_vec())
    );
    assert_eq!(
        c.call(&[b"LCS", b"lcs-a", b"lcs-b", b"IDX", b"MINMATCHLEN"]),
        Reply::Error(b"ERR syntax error".to_vec())
    );
    for invalid in [b"x".as_slice(), b"+2", b"02", b"-0", b""] {
        assert_eq!(
            c.call(&[b"LCS", b"lcs-a", b"lcs-b", b"IDX", b"MINMATCHLEN", invalid]),
            Reply::Error(b"ERR value is not an integer or out of range".to_vec())
        );
    }
    assert_eq!(
        c.call(&[b"LCS", b"lcs-a"]),
        Reply::Error(b"ERR wrong number of arguments for 'lcs' command".to_vec())
    );
    assert_eq!(
        c.call(&[b"LCS", b"lcs-a", b"lcs-list"]),
        Reply::Error(b"ERR The specified keys must contain string values".to_vec())
    );
    assert_eq!(
        c.call(&[b"LCS", b"lcs-list", b"lcs-a"]),
        Reply::Error(b"ERR The specified keys must contain string values".to_vec())
    );
    assert_eq!(
        c.call(&[b"LCS", b"lcs-a", b"lcs-b", b"WITHMATCHLEN"]),
        bulk(b"abc")
    );
    assert_eq!(
        c.call(&[b"LCS", b"lcs-a", b"lcs-b", b"MINMATCHLEN", b"2"]),
        bulk(b"abc")
    );
    assert_eq!(
        c.call(&[b"LCS", b"lcs-a", b"lcs-b", b"IDX", b"MINMATCHLEN", b"-1"]),
        array(vec![
            bulk(b"matches"),
            array(vec![array(vec![
                array(vec![integer(0), integer(2)]),
                array(vec![integer(0), integer(2)]),
            ])]),
            bulk(b"len"),
            integer(3),
        ])
    );
}

#[test]
fn lcs_bounds_index_matrix_memory() {
    let server = common::Server::start();
    let mut c = Client::new(&server);
    let value = vec![b'a'; 3000];
    assert_eq!(c.call(&[b"SET", b"lcs-big-one", &value]), ok());
    assert_eq!(c.call(&[b"SET", b"lcs-big-two", &value]), ok());
    assert_eq!(
        c.call(&[b"LCS", b"lcs-big-one", b"lcs-big-two"]),
        Reply::Error(b"ERR LCS keys are too large".to_vec())
    );
    assert_eq!(
        c.call(&[b"LCS", b"lcs-big-one", b"lcs-big-two", b"LEN"]),
        Reply::Error(b"ERR LCS keys are too large".to_vec())
    );
    assert_eq!(
        c.call(&[b"LCS", b"lcs-big-one", b"lcs-big-two", b"IDX"]),
        Reply::Error(b"ERR LCS keys are too large".to_vec())
    );
    let within = vec![b'a'; 2800];
    assert_eq!(c.call(&[b"SET", b"lcs-fit-one", &within]), ok());
    assert_eq!(c.call(&[b"SET", b"lcs-fit-two", &within]), ok());
    assert_eq!(
        c.call(&[b"LCS", b"lcs-fit-one", b"lcs-fit-two", b"LEN"]),
        integer(2800)
    );
}

#[test]
fn lcs_checks_types_before_options_and_handles_expiry_and_multi_transactions() {
    let server = common::Server::start();
    let mut c = Client::new(&server);
    assert_eq!(c.call(&[b"SET", b"lcs-text", b"value"]), ok());
    assert_eq!(c.call(&[b"RPUSH", b"lcs-wrong", b"x"]), integer(1));
    for command in [
        &[b"LCS".as_slice(), b"lcs-text", b"lcs-wrong", b"FOO"] as &[&[u8]],
        &[b"LCS", b"lcs-wrong", b"lcs-text", b"LEN", b"IDX"],
        &[
            b"LCS",
            b"lcs-wrong",
            b"lcs-text",
            b"IDX",
            b"MINMATCHLEN",
            b"x",
        ],
    ] {
        assert_eq!(
            c.call(command),
            Reply::Error(b"ERR The specified keys must contain string values".to_vec())
        );
    }
    assert_eq!(
        c.call(&[b"LCS", b"lcs-text", b"lcs-text", b"IDX", b"LEN"]),
        Reply::Error(b"ERR If you want both the length and indexes, please just use IDX.".to_vec())
    );
    // The last MINMATCHLEN wins: 6 then 0 keeps the match, 0 then 6 drops it.
    assert_eq!(
        c.call(&[
            b"LCS",
            b"lcs-text",
            b"lcs-text",
            b"IDX",
            b"MINMATCHLEN",
            b"6",
            b"MINMATCHLEN",
            b"0",
        ]),
        array(vec![
            bulk(b"matches"),
            array(vec![array(vec![
                array(vec![integer(0), integer(4)]),
                array(vec![integer(0), integer(4)]),
            ])]),
            bulk(b"len"),
            integer(5),
        ])
    );
    assert_eq!(
        c.call(&[
            b"LCS",
            b"lcs-text",
            b"lcs-text",
            b"IDX",
            b"MINMATCHLEN",
            b"0",
            b"MINMATCHLEN",
            b"6",
        ]),
        array(vec![
            bulk(b"matches"),
            array(vec![]),
            bulk(b"len"),
            integer(5),
        ])
    );
    assert_eq!(
        c.call(&[b"SET", b"lcs-expiring", b"value", b"PX", b"10"]),
        ok()
    );
    std::thread::sleep(Duration::from_millis(40));
    assert_eq!(c.call(&[b"LCS", b"lcs-expiring", b"lcs-text"]), bulk(b""));
    assert_eq!(
        c.call(&[b"LCS", b"lcs-expiring", b"lcs-text", b"LEN"]),
        integer(0)
    );
    assert_eq!(c.call(&[b"MULTI"]), ok());
    assert_eq!(
        c.call(&[b"LCS", b"lcs-text", b"lcs-text", b"LEN"]),
        queued()
    );
    assert_eq!(c.call(&[b"EXEC"]), array(vec![integer(5)]));
}

#[test]
fn large_replies_survive_a_slow_reader() {
    use std::io::{Read, Write};
    use std::net::TcpStream;

    fn frame(args: &[&[u8]]) -> Vec<u8> {
        let mut bytes = format!("*{}\r\n", args.len()).into_bytes();
        for arg in args {
            bytes.extend(format!("${}\r\n", arg.len()).as_bytes());
            bytes.extend(*arg);
            bytes.extend(b"\r\n");
        }
        bytes
    }

    let server = common::Server::start();
    let mut socket = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let value = vec![b'v'; 8 * 1024 * 1024];
    socket
        .write_all(&frame(&[b"SET", b"big-slow", &value]))
        .unwrap();
    let mut reply = [0u8; 16];
    let read = socket.read(&mut reply).unwrap();
    assert_eq!(&reply[..read], b"+OK\r\n");

    socket.write_all(&frame(&[b"GET", b"big-slow"])).unwrap();
    std::thread::sleep(Duration::from_millis(1200));
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 65536];
    while bytes.len() < value.len() + 12 {
        match socket.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => bytes.extend_from_slice(&chunk[..read]),
            Err(error) => panic!("read failed after {} bytes: {error}", bytes.len()),
        }
    }
    assert_eq!(bytes.len(), value.len() + 12);
    assert!(bytes.starts_with(b"$8388608\r\n"));
    assert!(bytes.ends_with(b"\r\n"));
}

#[test]
fn ranges_clamp_very_negative_end_indexes() {
    let server = common::Server::start();
    let mut c = Client::new(&server);
    assert_eq!(c.call(&[b"SET", b"range", b"Hello World"]), ok());
    assert_eq!(c.call(&[b"GETRANGE", b"range", b"0", b"-100"]), bulk(b"H"));
    assert_eq!(c.call(&[b"SUBSTR", b"range", b"0", b"-100"]), bulk(b"H"));
    // Both indexes negative and start > end is empty before conversion.
    assert_eq!(c.call(&[b"GETRANGE", b"range", b"-11", b"-12"]), bulk(b""));
    assert_eq!(
        c.call(&[b"GETRANGE", b"range", b"-100", b"-101"]),
        bulk(b"")
    );
    assert_eq!(
        c.call(&[b"GETRANGE", b"missing-range", b"0", b"-1"]),
        bulk(b"")
    );
    assert_eq!(c.call(&[b"SET", b"empty-range", b""]), ok());
    assert_eq!(
        c.call(&[b"GETRANGE", b"empty-range", b"0", b"-1"]),
        bulk(b"")
    );
}

#[test]
fn set_and_getex_accept_repeated_options_like_redis() {
    let server = common::Server::start();
    let mut c = Client::new(&server);
    assert_eq!(c.call(&[b"SET", b"dup", b"v", b"NX", b"NX"]), ok());
    assert_eq!(c.call(&[b"SET", b"dup", b"v", b"XX", b"XX"]), ok());
    assert_eq!(
        c.call(&[b"SET", b"dup", b"next", b"GET", b"GET"]),
        bulk(b"v")
    );
    assert_eq!(
        c.call(&[b"SET", b"dup", b"v", b"KEEPTTL", b"KEEPTTL"]),
        ok()
    );
    assert_eq!(
        c.call(&[b"SET", b"dup", b"v", b"EX", b"10", b"EX", b"20"]),
        ok()
    );
    assert_eq!(c.call(&[b"TTL", b"dup"]), integer(20));
    for command in [
        &[b"SET".as_slice(), b"dup", b"v", b"EX", b"10", b"KEEPTTL"] as &[&[u8]],
        &[b"SET", b"dup", b"v", b"KEEPTTL", b"EX", b"10"],
        &[b"SET", b"dup", b"v", b"EX", b"10", b"PX", b"5000"],
        &[b"SET", b"dup", b"v", b"EX", b"10", b"EXAT", b"9999999999"],
        &[b"SET", b"dup", b"v", b"FOO"],
    ] {
        assert_eq!(c.call(command), Reply::Error(b"ERR syntax error".to_vec()));
    }
    assert_eq!(
        c.call(&[b"GETEX", b"dup", b"EX", b"10", b"EX", b"20"]),
        bulk(b"v")
    );
    assert_eq!(c.call(&[b"TTL", b"dup"]), integer(20));
    assert_eq!(
        c.call(&[b"GETEX", b"dup", b"PERSIST", b"PERSIST"]),
        bulk(b"v")
    );
    assert_eq!(c.call(&[b"TTL", b"dup"]), integer(-1));
    for command in [
        &[b"GETEX".as_slice(), b"dup", b"EX", b"10", b"PERSIST"] as &[&[u8]],
        &[b"GETEX", b"dup", b"PERSIST", b"EX", b"10"],
        &[b"GETEX", b"dup", b"EX", b"10", b"PX", b"5000"],
        &[b"GETEX", b"dup", b"FOO"],
    ] {
        assert_eq!(c.call(command), Reply::Error(b"ERR syntax error".to_vec()));
    }
}

#[test]
fn expiry_parse_errors_match_redis_text() {
    let server = common::Server::start();
    let mut c = Client::new(&server);
    assert_eq!(c.call(&[b"SET", b"k", b"v"]), ok());
    let integer = Reply::Error(b"ERR value is not an integer or out of range".to_vec());
    for command in [
        &[b"SET".as_slice(), b"k", b"v", b"EX", b"abc"] as &[&[u8]],
        &[b"SET", b"k", b"v", b"EX", b"1.5"],
        &[b"SET", b"k", b"v", b"EX", b"99999999999999999999"],
        &[b"SET", b"k", b"v", b"EX", b"9223372036854775808"],
        &[b"SET", b"k", b"v", b"EX", b"+1"],
        &[b"SET", b"k", b"v", b"EX", b"01"],
        &[b"SET", b"k", b"v", b"EX", b"-0"],
        &[b"SET", b"k", b"v", b"EXAT", b"01"],
        &[b"SETEX", b"k", b"1.5", b"v"],
        &[b"PSETEX", b"k", b"1.5", b"v"],
        &[b"GETEX", b"k", b"EX", b"abc"],
        &[b"GETEX", b"k", b"PX", b"1.5"],
        &[b"GETEX", b"k", b"EXAT", b"+1"],
    ] {
        assert_eq!(c.call(command), integer, "{command:?}");
    }
    let before = c.call(&[b"GET", b"k"]);
    for (command, name) in [
        (
            &[b"SET".as_slice(), b"k", b"v", b"EX", b"0"] as &[&[u8]],
            "set",
        ),
        (&[b"SET", b"k", b"v", b"EX", b"-1"], "set"),
        (&[b"SETEX", b"k", b"0", b"v"], "setex"),
        (&[b"PSETEX", b"k", b"-1", b"v"], "psetex"),
        (&[b"GETEX", b"k", b"EX", b"0"], "getex"),
        (&[b"GETEX", b"k", b"EX", b"-1"], "getex"),
        (&[b"SET", b"k", b"v", b"EX", b"9223372036854776"], "set"),
        (&[b"SETEX", b"k", b"9223372036854776", b"v"], "setex"),
        (&[b"SET", b"k", b"v", b"EXAT", b"0"], "set"),
    ] {
        assert_eq!(
            c.call(command),
            Reply::Error(format!("ERR invalid expire time in '{name}' command").into_bytes()),
            "{command:?}"
        );
        assert_eq!(c.call(&[b"GET", b"k"]), before, "{command:?}");
    }
}
