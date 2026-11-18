// SPDX-License-Identifier: AGPL-3.0-only
#[path = "common/client.rs"]
mod client;
mod common;
use client::{array, integer, ok, queued, Client, Reply};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn bulk(value: &[u8]) -> Reply {
    Reply::Bulk(Some(value.to_vec()))
}
fn members(values: &[&[u8]]) -> Reply {
    array(values.iter().map(|value| bulk(value)).collect())
}
fn wrong_type() -> Reply {
    Reply::Error(b"WRONGTYPE Operation against a key holding the wrong kind of value".to_vec())
}

#[test]
fn hash_field_expiry_reports_per_field_results_and_errors() {
    let server = common::Server::start();
    let mut c = Client::new(&server);
    assert_eq!(
        c.call(&[b"HSET", b"h", b"a\0", b"one", b"b", b"two"]),
        integer(2)
    );
    assert_eq!(
        c.call(&[
            b"HPEXPIRE",
            b"h",
            b"5000",
            b"FIELDS",
            b"3",
            b"a\0",
            b"b",
            b"absent"
        ]),
        array(vec![integer(1), integer(1), integer(-2)])
    );
    assert!(matches!(
        c.call(&[b"HPTTL", b"h", b"FIELDS", b"3", b"a\0", b"b", b"absent"]),
        Reply::Array(Some(values)) if matches!(values.as_slice(), [Reply::Integer(0..=5000), Reply::Integer(0..=5000), Reply::Integer(-2)])
    ));
    assert_eq!(
        c.call(&[b"HPERSIST", b"h", b"FIELDS", b"2", b"a\0", b"absent"]),
        array(vec![integer(1), integer(-2)])
    );
    assert_eq!(
        c.call(&[b"HTTL", b"h", b"FIELDS", b"1", b"a\0"]),
        array(vec![integer(-1)])
    );
    assert_eq!(
        c.call(&[b"HPEXPIRE", b"h", b"0", b"FIELDS", b"1", b"b"]),
        array(vec![integer(2)])
    );
    assert_eq!(c.call(&[b"HGET", b"h", b"b"]), Reply::Bulk(None));
    assert_eq!(c.call(&[b"SET", b"string", b"v"]), ok());
    for command in [
        &[
            b"HEXPIRE".as_slice(),
            b"string",
            b"5",
            b"FIELDS",
            b"1",
            b"a",
        ] as &[&[u8]],
        &[b"HPEXPIRE", b"string", b"5", b"FIELDS", b"1", b"a"],
        &[
            b"HEXPIREAT",
            b"string",
            b"2000000000",
            b"FIELDS",
            b"1",
            b"a",
        ],
        &[
            b"HPEXPIREAT",
            b"string",
            b"2000000000000",
            b"FIELDS",
            b"1",
            b"a",
        ],
        &[b"HTTL", b"string", b"FIELDS", b"1", b"a"],
        &[b"HPTTL", b"string", b"FIELDS", b"1", b"a"],
        &[b"HEXPIRETIME", b"string", b"FIELDS", b"1", b"a"],
        &[b"HPEXPIRETIME", b"string", b"FIELDS", b"1", b"a"],
        &[b"HPERSIST", b"string", b"FIELDS", b"1", b"a"],
        &[b"HGETEX", b"string", b"FIELDS", b"1", b"a"],
        &[b"HSETEX", b"string", b"FIELDS", b"1", b"a", b"x"],
    ] {
        assert_eq!(c.call(command), wrong_type(), "{command:?}");
    }
    assert!(matches!(
        c.call(&[b"HEXPIRE", b"h", b"bad", b"FIELDS", b"1", b"a"]),
        Reply::Error(_)
    ));
    assert_eq!(
        c.call(&[b"HEXPIRE", b"h", b"10", b"NX", b"XX", b"FIELDS", b"1", b"a"]),
        Reply::Error(b"ERR Multiple condition flags specified".to_vec())
    );
    assert_eq!(
        c.call(&[b"HGETEX", b"h", b"PERSIST", b"EX", b"10", b"FIELDS", b"1", b"a"]),
        Reply::Error(
            b"ERR Only one of EX, PX, EXAT, PXAT or PERSIST arguments can be specified".to_vec()
        )
    );
    assert_eq!(
        c.call(&[b"HSETEX", b"h", b"FNX", b"FXX", b"FIELDS", b"1", b"a", b"x"]),
        Reply::Error(b"ERR Only one of FXX or FNX arguments can be specified".to_vec())
    );
}

#[test]
fn hash_field_expiry_conditions_and_overwrite_rules() {
    let server = common::Server::start();
    let mut c = Client::new(&server);
    assert_eq!(c.call(&[b"HSET", b"h", b"a", b"1", b"b", b"2"]), integer(2));
    assert_eq!(
        c.call(&[b"HEXPIRE", b"h", b"30", b"XX", b"FIELDS", b"2", b"a", b"b"]),
        array(vec![integer(0), integer(0)])
    );
    assert_eq!(
        c.call(&[b"HEXPIRE", b"h", b"30", b"NX", b"FIELDS", b"2", b"a", b"b"]),
        array(vec![integer(1), integer(1)])
    );
    assert_eq!(
        c.call(&[b"HEXPIRE", b"h", b"60", b"GT", b"FIELDS", b"1", b"a"]),
        array(vec![integer(1)])
    );
    assert_eq!(
        c.call(&[b"HEXPIRE", b"h", b"10", b"GT", b"FIELDS", b"1", b"a"]),
        array(vec![integer(0)])
    );
    assert_eq!(
        c.call(&[b"HEXPIRE", b"h", b"10", b"LT", b"FIELDS", b"1", b"a"]),
        array(vec![integer(1)])
    );
    assert_eq!(c.call(&[b"HINCRBY", b"h", b"a", b"2"]), integer(3));
    assert!(
        matches!(c.call(&[b"HTTL", b"h", b"FIELDS", b"1", b"a"]), Reply::Array(Some(values)) if matches!(values[0], Reply::Integer(n) if (0..=10).contains(&n)))
    );
    assert_eq!(c.call(&[b"HSET", b"h", b"a", b"new"]), integer(0));
    assert_eq!(
        c.call(&[b"HTTL", b"h", b"FIELDS", b"1", b"a"]),
        array(vec![integer(-1)])
    );
    assert_eq!(
        c.call(&[b"HPERSIST", b"h", b"FIELDS", b"2", b"a", b"b"]),
        array(vec![integer(-1), integer(1)])
    );
}

#[test]
fn hash_field_absolute_expiry_is_reported_exactly() {
    let server = common::Server::start();
    let mut c = Client::new(&server);
    let at = (SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis()
        + 100_000)
        .to_string();
    assert_eq!(c.call(&[b"HSET", b"h", b"field", b"value"]), integer(1));
    assert_eq!(
        c.call(&[
            b"HPEXPIREAT",
            b"h",
            at.as_bytes(),
            b"FIELDS",
            b"1",
            b"field"
        ]),
        array(vec![integer(1)])
    );
    assert_eq!(
        c.call(&[b"HPEXPIRETIME", b"h", b"FIELDS", b"1", b"field"]),
        array(vec![integer(at.parse().unwrap())])
    );
    let seconds = (at.parse::<i64>().unwrap() / 1000).to_string();
    assert_eq!(
        c.call(&[
            b"HEXPIREAT",
            b"h",
            seconds.as_bytes(),
            b"FIELDS",
            b"1",
            b"field",
        ]),
        array(vec![integer(1)])
    );
    assert_eq!(
        c.call(&[b"HEXPIRETIME", b"h", b"FIELDS", b"1", b"field"]),
        array(vec![integer(seconds.parse().unwrap())])
    );
}

#[test]
fn hgetex_and_hsetex_update_fields_atomically() {
    let server = common::Server::start();
    let mut c = Client::new(&server);
    assert_eq!(
        c.call(&[b"HSETEX", b"h", b"EX", b"50", b"FIELDS", b"2", b"a", b"one", b"b", b"two"]),
        integer(1)
    );
    assert_eq!(
        c.call(&[b"HSETEX", b"h", b"FNX", b"FIELDS", b"2", b"a", b"wrong", b"c", b"three"]),
        integer(0)
    );
    assert_eq!(
        c.call(&[
            b"HSETEX", b"h", b"FXX", b"KEEPTTL", b"FIELDS", b"2", b"a", b"new", b"b", b"updated"
        ]),
        integer(1)
    );
    assert_eq!(
        c.call(&[b"HGETEX", b"h", b"PERSIST", b"FIELDS", b"3", b"a", b"b", b"missing"]),
        array(vec![bulk(b"new"), bulk(b"updated"), Reply::Bulk(None)])
    );
    assert_eq!(
        c.call(&[b"HTTL", b"h", b"FIELDS", b"2", b"a", b"b"]),
        array(vec![integer(-1), integer(-1)])
    );
    assert_eq!(
        c.call(&[b"HGETEX", b"h", b"PX", b"3000", b"FIELDS", b"1", b"a"]),
        array(vec![bulk(b"new")])
    );
    assert_eq!(
        c.call(&[b"HSETEX", b"h", b"FIELDS", b"1", b"a", b"overwritten"]),
        integer(1)
    );
    assert_eq!(
        c.call(&[b"HPTTL", b"h", b"FIELDS", b"1", b"a"]),
        array(vec![integer(-1)])
    );
    assert_eq!(
        c.call(&[b"HGETEX", b"h", b"FIELDS", b"1", b"missing"]),
        array(vec![Reply::Bulk(None)])
    );
    assert!(matches!(
        c.call(&[b"HSETEX", b"h", b"EX", b"5", b"KEEPTTL", b"FIELDS", b"1", b"a", b"x"]),
        Reply::Error(_)
    ));
}

#[test]
fn hash_field_expiry_survives_snapshot_and_aof_restarts() {
    let directory = common::temp_path("reru-hfield", "");
    std::fs::create_dir(&directory).unwrap();
    for (mode, suffix) in [("--snapshot", "snap"), ("--appendonly", "aof")] {
        let path = directory.join(suffix);
        let filename = path.to_str().unwrap();
        {
            let server = common::Server::start_with(&[mode, filename]);
            let mut c = Client::new(&server);
            assert_eq!(
                c.call(&[b"HSET", b"h", b"volatile", b"v", b"stable", b"s"]),
                integer(2)
            );
            assert_eq!(
                c.call(&[b"HPEXPIRE", b"h", b"100000", b"FIELDS", b"1", b"volatile"]),
                array(vec![integer(1)])
            );
        }
        {
            let server = common::Server::start_with(&[mode, filename]);
            let mut c = Client::new(&server);
            assert!(
                matches!(c.call(&[b"HPTTL", b"h", b"FIELDS", b"1", b"volatile"]), Reply::Array(Some(values)) if matches!(values[0], Reply::Integer(n) if (0..=100000).contains(&n)))
            );
            assert_eq!(
                c.call(&[b"HPEXPIREAT", b"h", b"1", b"FIELDS", b"1", b"volatile"]),
                array(vec![integer(2)])
            );
            assert_eq!(c.call(&[b"HGETALL", b"h"]), members(&[b"stable", b"s"]));
        }
        {
            let server = common::Server::start_with(&[mode, filename]);
            let mut c = Client::new(&server);
            assert_eq!(c.call(&[b"HGETALL", b"h"]), members(&[b"stable", b"s"]));
            assert_eq!(
                c.call(&[b"HPTTL", b"h", b"FIELDS", b"1", b"volatile"]),
                array(vec![integer(-2)])
            );
        }
    }
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn idle_field_expiry_invalidates_watch_and_rewrites_aof() {
    let path = common::temp_path("reru-hfield-active", ".aof");
    let filename = path.to_str().unwrap();
    {
        let server = common::Server::start_with(&["--appendonly", filename]);
        let mut writer = Client::new(&server);
        let mut watcher = Client::new(&server);
        assert_eq!(
            writer.call(&[b"HSET", b"h", b"short", b"x", b"long", b"y"]),
            integer(2)
        );
        assert_eq!(
            writer.call(&[b"HPEXPIRE", b"h", b"100", b"FIELDS", b"1", b"short"]),
            array(vec![integer(1)])
        );
        assert_eq!(watcher.call(&[b"WATCH", b"h"]), ok());
        let before = std::fs::metadata(&path).unwrap().len();
        let until = Instant::now() + Duration::from_secs(3);
        while std::fs::metadata(&path).unwrap().len() == before && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            std::fs::metadata(&path).unwrap().len() > before,
            "background expiry did not persist"
        );
        assert_eq!(watcher.call(&[b"MULTI"]), ok());
        assert_eq!(watcher.call(&[b"HGET", b"h", b"long"]), queued());
        assert_eq!(watcher.call(&[b"EXEC"]), Reply::Array(None));
        assert_eq!(writer.call(&[b"HGETALL", b"h"]), members(&[b"long", b"y"]));
        assert_eq!(
            writer.call(&[b"BGREWRITEAOF"]),
            Reply::Simple(b"Background append only file rewriting started".to_vec())
        );
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            let Reply::Bulk(Some(info)) = writer.call(&[b"INFO", b"persistence"]) else {
                panic!("missing INFO")
            };
            if info
                .windows(b"aof_rewrites:1\r\n".len())
                .any(|part| part == b"aof_rewrites:1\r\n")
            {
                break;
            }
            assert!(Instant::now() < until, "AOF rewrite did not complete");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    {
        let server = common::Server::start_with(&["--appendonly", filename]);
        let mut c = Client::new(&server);
        assert_eq!(c.call(&[b"HGETALL", b"h"]), members(&[b"long", b"y"]));
        assert_eq!(
            c.call(&[b"HPTTL", b"h", b"FIELDS", b"1", b"short"]),
            array(vec![integer(-2)])
        );
    }
    std::fs::remove_file(path).unwrap();
}

#[test]
fn field_expiry_respects_key_ttl_and_reaches_replicas() {
    let primary = common::Server::start();
    let mut c = Client::new(&primary);
    assert_eq!(
        c.call(&[b"HSET", b"h", b"soon", b"x", b"later", b"y"]),
        integer(2)
    );
    assert_eq!(c.call(&[b"PEXPIRE", b"h", b"100000"]), integer(1));
    assert_eq!(
        c.call(&[b"HPEXPIRE", b"h", b"5000", b"FIELDS", b"1", b"soon"]),
        array(vec![integer(1)])
    );
    let replica =
        common::Server::start_with(&["--replicaof", "127.0.0.1", &primary.port.to_string()]);
    let mut reader = Client::new(&replica);
    let until = Instant::now() + Duration::from_secs(2);
    while reader.call(&[b"HGET", b"h", b"soon"]) != bulk(b"x") && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(reader.call(&[b"HGET", b"h", b"soon"]), bulk(b"x"));
    assert!(
        matches!(reader.call(&[b"HPTTL", b"h", b"FIELDS", b"1", b"soon"]), Reply::Array(Some(values)) if matches!(values[0], Reply::Integer(n) if n > 0 && n <= 5000))
    );
    assert!(
        matches!(reader.call(&[b"HPEXPIRE", b"h", b"10", b"FIELDS", b"1", b"soon"]), Reply::Error(e) if e.starts_with(b"READONLY"))
    );
    assert!(matches!(reader.call(&[b"PTTL", b"h"]), Reply::Integer(n) if n > 0 && n <= 100000));
}

#[test]
fn hash_fields_are_binary_safe_and_updates_count_only_new_fields() {
    let server = common::Server::start();
    let mut c = Client::new(&server);
    assert_eq!(
        c.call(&[b"hset", b"h\0", b"f\0", b"v\r\n\0", b"f\0", b"last", b"", b""]),
        integer(2)
    );
    assert_eq!(c.call(&[b"TYPE", b"h\0"]), Reply::Simple(b"hash".to_vec()));
    assert_eq!(c.call(&[b"HGET", b"h\0", b"f\0"]), bulk(b"last"));
    assert_eq!(c.call(&[b"HGET", b"h\0", b""]), bulk(b""));
    assert_eq!(c.call(&[b"HGET", b"h\0", b"missing"]), Reply::Bulk(None));
    assert_eq!(c.call(&[b"HMSET", b"h\0", b"f\0", b"v\r\n\0"]), ok());
    assert_eq!(
        c.call(&[b"HMGET", b"h\0", b"missing", b"f\0", b"f\0", b""]),
        array(vec![
            Reply::Bulk(None),
            bulk(b"v\r\n\0"),
            bulk(b"v\r\n\0"),
            bulk(b"")
        ])
    );
    assert_eq!(c.call(&[b"HLEN", b"h\0"]), integer(2));
    assert_eq!(c.call(&[b"HEXISTS", b"h\0", b""]), integer(1));
    assert_eq!(c.call(&[b"HEXISTS", b"h\0", b"missing"]), integer(0));
}

#[test]
fn hash_enumeration_deletion_and_setnx_preserve_expected_values() {
    let server = common::Server::start();
    let mut c = Client::new(&server);
    assert_eq!(c.call(&[b"HSETNX", b"h", b"b", b"2"]), integer(1));
    assert_eq!(c.call(&[b"HSETNX", b"h", b"b", b"ignored"]), integer(0));
    assert_eq!(c.call(&[b"HSET", b"h", b"a", b"1"]), integer(1));
    assert_eq!(
        c.call(&[b"HGETALL", b"h"]),
        members(&[b"a", b"1", b"b", b"2"])
    );
    assert_eq!(c.call(&[b"HKEYS", b"h"]), members(&[b"a", b"b"]));
    assert_eq!(c.call(&[b"HVALS", b"h"]), members(&[b"1", b"2"]));
    assert_eq!(c.call(&[b"HDEL", b"h", b"a", b"a", b"unknown"]), integer(1));
    assert_eq!(c.call(&[b"HDEL", b"h", b"b"]), integer(1));
    assert_eq!(c.call(&[b"TYPE", b"h"]), Reply::Simple(b"none".to_vec()));
    for name in [b"HGETALL".as_slice(), b"HKEYS", b"HVALS"] {
        assert_eq!(c.call(&[name, b"h"]), array(vec![]));
    }
    assert_eq!(c.call(&[b"HLEN", b"h"]), integer(0));
    assert_eq!(c.call(&[b"HEXISTS", b"h", b"b"]), integer(0));
    assert_eq!(c.call(&[b"HDEL", b"h", b"b"]), integer(0));
    assert_eq!(
        c.call(&[b"HMGET", b"h", b"a", b"b"]),
        array(vec![Reply::Bulk(None), Reply::Bulk(None)])
    );
}

#[test]
fn hash_increment_checks_signed_boundaries_without_mutating_on_errors() {
    let server = common::Server::start();
    let mut c = Client::new(&server);
    assert_eq!(c.call(&[b"HINCRBY", b"h", b"n", b"-5"]), integer(-5));
    assert_eq!(c.call(&[b"HINCRBY", b"h", b"n", b"6"]), integer(1));
    assert_eq!(
        c.call(&[
            b"HSET",
            b"h",
            b"max",
            b"9223372036854775807",
            b"min",
            b"-9223372036854775808",
            b"bad",
            b"01"
        ]),
        integer(3)
    );
    for (field, delta) in [(b"max".as_slice(), b"1".as_slice()), (b"min", b"-1")] {
        assert_eq!(
            c.call(&[b"HINCRBY", b"h", field, delta]),
            Reply::Error(b"ERR increment or decrement would overflow".to_vec())
        );
    }
    assert_eq!(
        c.call(&[b"HGET", b"h", b"max"]),
        bulk(b"9223372036854775807")
    );
    assert_eq!(
        c.call(&[b"HGET", b"h", b"min"]),
        bulk(b"-9223372036854775808")
    );
    assert_eq!(
        c.call(&[b"HINCRBY", b"h", b"bad", b"1"]),
        Reply::Error(b"ERR hash value is not an integer".to_vec())
    );
    for delta in [b"+1".as_slice(), b"01", b"1.5", b"9223372036854775808"] {
        assert!(matches!(
            c.call(&[b"HINCRBY", b"missing", b"n", delta]),
            Reply::Error(_)
        ));
    }
    assert_eq!(c.call(&[b"EXISTS", b"missing"]), integer(0));
}

#[test]
fn set_membership_deduplication_and_last_member_removal() {
    let server = common::Server::start();
    let mut c = Client::new(&server);
    assert_eq!(
        c.call(&[b"sadd", b"s", b"a\0", b"a\0", b"", b"b\r\n"]),
        integer(3)
    );
    assert_eq!(c.call(&[b"TYPE", b"s"]), Reply::Simple(b"set".to_vec()));
    assert_eq!(c.call(&[b"SADD", b"s", b"a\0"]), integer(0));
    assert_eq!(c.call(&[b"SCARD", b"s"]), integer(3));
    assert_eq!(c.call(&[b"SISMEMBER", b"s", b"a\0"]), integer(1));
    assert_eq!(c.call(&[b"SISMEMBER", b"s", b"missing"]), integer(0));
    assert_eq!(
        c.call(&[b"SMISMEMBER", b"s", b"", b"missing", b"a\0", b"a\0"]),
        array(vec![integer(1), integer(0), integer(1), integer(1)])
    );
    assert_eq!(
        c.call(&[b"SMEMBERS", b"s"]),
        members(&[b"", b"a\0", b"b\r\n"])
    );
    assert_eq!(
        c.call(&[b"SREM", b"s", b"a\0", b"a\0", b"missing"]),
        integer(1)
    );
    assert_eq!(c.call(&[b"SREM", b"s", b"", b"b\r\n"]), integer(2));
    assert_eq!(c.call(&[b"TYPE", b"s"]), Reply::Simple(b"none".to_vec()));
    assert_eq!(c.call(&[b"SCARD", b"s"]), integer(0));
    assert_eq!(c.call(&[b"SMEMBERS", b"s"]), array(vec![]));
    assert_eq!(c.call(&[b"SREM", b"s", b"a"]), integer(0));
    assert_eq!(
        c.call(&[b"SMISMEMBER", b"s", b"a", b"b"]),
        array(vec![integer(0), integer(0)])
    );
}

#[test]
fn set_algebra_handles_missing_and_repeated_inputs() {
    let server = common::Server::start();
    let mut c = Client::new(&server);
    assert_eq!(c.call(&[b"SADD", b"a", b"1", b"2"]), integer(2));
    assert_eq!(c.call(&[b"SADD", b"b", b"2", b"3"]), integer(2));
    assert_eq!(
        c.call(&[b"SUNION", b"a", b"missing", b"b", b"a"]),
        members(&[b"1", b"2", b"3"])
    );
    assert_eq!(c.call(&[b"SINTER", b"a", b"b", b"a"]), members(&[b"2"]));
    assert_eq!(c.call(&[b"SINTER", b"a", b"missing"]), array(vec![]));
    assert_eq!(
        c.call(&[b"SDIFF", b"a", b"missing", b"b"]),
        members(&[b"1"])
    );
    assert_eq!(c.call(&[b"SDIFF", b"a", b"a"]), array(vec![]));
    assert_eq!(c.call(&[b"SDIFF", b"missing", b"a"]), array(vec![]));
    for name in [b"SUNION".as_slice(), b"SINTER", b"SDIFF"] {
        assert_eq!(c.call(&[name, b"a"]), members(&[b"1", b"2"]));
    }
}

#[test]
fn collection_commands_reject_wrong_types_and_bad_arity_without_changes() {
    let server = common::Server::start();
    let mut c = Client::new(&server);
    assert_eq!(c.call(&[b"SET", b"k", b"v"]), ok());
    let commands: &[&[&[u8]]] = &[
        &[b"HSET", b"k", b"f", b"v"],
        &[b"HMSET", b"k", b"f", b"v"],
        &[b"HSETNX", b"k", b"f", b"v"],
        &[b"HINCRBY", b"k", b"f", b"1"],
        &[b"HGET", b"k", b"f"],
        &[b"HMGET", b"k", b"f"],
        &[b"HGETALL", b"k"],
        &[b"HLEN", b"k"],
        &[b"HEXISTS", b"k", b"f"],
        &[b"HKEYS", b"k"],
        &[b"HVALS", b"k"],
        &[b"HDEL", b"k", b"f"],
        &[b"SADD", b"k", b"m"],
        &[b"SREM", b"k", b"m"],
        &[b"SCARD", b"k"],
        &[b"SMEMBERS", b"k"],
        &[b"SISMEMBER", b"k", b"m"],
        &[b"SMISMEMBER", b"k", b"m"],
        &[b"SUNION", b"k"],
        &[b"SINTER", b"k"],
        &[b"SDIFF", b"k"],
    ];
    for command in commands {
        assert_eq!(c.call(command), wrong_type());
        assert!(matches!(c.call(&[command[0]]), Reply::Error(_)));
    }
    for name in [b"SUNION".as_slice(), b"SINTER", b"SDIFF"] {
        assert_eq!(c.call(&[name, b"missing", b"k"]), wrong_type());
    }
    assert!(matches!(
        c.call(&[b"HSET", b"missing", b"f", b"v", b"dangling"]),
        Reply::Error(_)
    ));
    assert_eq!(c.call(&[b"EXISTS", b"missing"]), integer(0));
    assert_eq!(c.call(&[b"GET", b"k"]), bulk(b"v"));
}

#[test]
fn collection_mutations_preserve_expiry_and_expired_keys_can_change_type() {
    let server = common::Server::start();
    let mut c = Client::new(&server);
    assert_eq!(c.call(&[b"HSET", b"h", b"f", b"v"]), integer(1));
    assert_eq!(c.call(&[b"SADD", b"s", b"a"]), integer(1));
    for key in [b"h".as_slice(), b"s"] {
        assert_eq!(c.call(&[b"PEXPIRE", key, b"10000"]), integer(1));
    }
    assert_eq!(c.call(&[b"HINCRBY", b"h", b"n", b"1"]), integer(1));
    assert_eq!(c.call(&[b"HSET", b"h", b"f", b"updated"]), integer(0));
    assert_eq!(c.call(&[b"SADD", b"s", b"b"]), integer(1));
    assert_eq!(c.call(&[b"SREM", b"s", b"a"]), integer(1));
    for key in [b"h".as_slice(), b"s"] {
        let Reply::Integer(ttl) = c.call(&[b"PTTL", key]) else {
            panic!("expected TTL")
        };
        assert!((5000..=10000).contains(&ttl));
        assert_eq!(c.call(&[b"PEXPIRE", key, b"1"]), integer(1));
    }
    std::thread::sleep(Duration::from_millis(20));
    assert_eq!(c.call(&[b"SADD", b"h", b"new"]), integer(1));
    assert_eq!(c.call(&[b"HSET", b"s", b"new", b"value"]), integer(1));
    assert_eq!(c.call(&[b"TTL", b"h"]), integer(-1));
    assert_eq!(c.call(&[b"TTL", b"s"]), integer(-1));
}

#[test]
fn watches_ignore_noop_collection_writes_but_track_real_and_same_value_writes() {
    let server = common::Server::start();
    let mut watcher = Client::new(&server);
    let mut writer = Client::new(&server);
    assert_eq!(writer.call(&[b"HSET", b"h", b"f", b"v"]), integer(1));
    assert_eq!(writer.call(&[b"SADD", b"s", b"a"]), integer(1));
    assert_eq!(watcher.call(&[b"WATCH", b"h", b"s"]), ok());
    assert_eq!(
        writer.call(&[b"HSETNX", b"h", b"f", b"ignored"]),
        integer(0)
    );
    assert_eq!(writer.call(&[b"HDEL", b"h", b"missing"]), integer(0));
    assert_eq!(writer.call(&[b"SADD", b"s", b"a", b"a"]), integer(0));
    assert_eq!(writer.call(&[b"SREM", b"s", b"missing"]), integer(0));
    assert!(matches!(
        writer.call(&[b"HINCRBY", b"h", b"f", b"1"]),
        Reply::Error(_)
    ));
    assert_eq!(watcher.call(&[b"MULTI"]), ok());
    assert_eq!(watcher.call(&[b"EXEC"]), array(vec![]));
    let mutations: &[&[&[u8]]] = &[
        &[b"HSET", b"h", b"f", b"v"],
        &[b"HMSET", b"h", b"f", b"v"],
        &[b"HSETNX", b"h", b"new", b"1"],
        &[b"HINCRBY", b"h", b"new", b"0"],
        &[b"HDEL", b"h", b"new"],
        &[b"SADD", b"s", b"b"],
        &[b"SREM", b"s", b"b"],
    ];
    for mutation in mutations {
        assert_eq!(watcher.call(&[b"WATCH", b"h", b"s"]), ok());
        assert!(!matches!(writer.call(mutation), Reply::Error(_)));
        assert_eq!(watcher.call(&[b"MULTI"]), ok());
        assert_eq!(watcher.call(&[b"EXEC"]), Reply::Array(None));
    }
}

#[test]
fn collections_execute_in_transactions_and_queue_errors_abort() {
    let server = common::Server::start();
    let mut c = Client::new(&server);
    assert_eq!(c.call(&[b"MULTI"]), ok());
    for command in [
        vec![b"HSET".as_slice(), b"h", b"n", b"1"],
        vec![b"HINCRBY", b"h", b"n", b"2"],
        vec![b"SADD", b"s", b"a"],
        vec![b"SISMEMBER", b"s", b"a"],
        vec![b"SADD", b"h", b"wrongtype"],
        vec![b"HGET", b"h", b"n"],
    ] {
        assert_eq!(c.call(&command), queued());
    }
    assert_eq!(
        c.call(&[b"EXEC"]),
        array(vec![
            integer(1),
            integer(3),
            integer(1),
            integer(1),
            wrong_type(),
            bulk(b"3")
        ])
    );
    assert_eq!(c.call(&[b"MULTI"]), ok());
    assert_eq!(
        c.call(&[b"HSET", b"should-not-exist", b"f", b"v"]),
        queued()
    );
    assert!(matches!(
        c.call(&[b"HSET", b"h", b"dangling"]),
        Reply::Error(_)
    ));
    assert!(matches!(c.call(&[b"EXEC"]), Reply::Error(_)));
    assert_eq!(c.call(&[b"EXISTS", b"should-not-exist"]), integer(0));
}

#[test]
fn concurrent_hash_increments_do_not_lose_updates() {
    let server = common::Server::start();
    let clients: Vec<_> = (0..4).map(|_| Client::new(&server)).collect();
    let workers: Vec<_> = clients
        .into_iter()
        .map(|mut c| {
            std::thread::spawn(move || {
                for _ in 0..100 {
                    assert!(matches!(
                        c.call(&[b"HINCRBY", b"counter", b"n", b"1"]),
                        Reply::Integer(_)
                    ));
                }
            })
        })
        .collect();
    for worker in workers {
        worker.join().unwrap();
    }
    let mut c = Client::new(&server);
    assert_eq!(c.call(&[b"HGET", b"counter", b"n"]), bulk(b"400"));
}

fn wait_for(c: &mut Client, command: &[&[u8]], expected: Reply) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if c.call(command) == expected {
            return;
        }
        assert!(Instant::now() < deadline, "replication timed out");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn replicas_receive_collections_and_watch_changes_and_reject_writes() {
    let primary = common::Server::start();
    let mut writer = Client::new(&primary);
    assert_eq!(writer.call(&[b"HSET", b"h", b"f\0", b"v\0"]), integer(1));
    assert_eq!(writer.call(&[b"SADD", b"s", b"m\0"]), integer(1));
    let port = primary.port.to_string();
    let replica = common::Server::start_with(&["--replicaof", "127.0.0.1", &port]);
    let mut reader = Client::new(&replica);
    wait_for(&mut reader, &[b"HGET", b"h", b"f\0"], bulk(b"v\0"));
    wait_for(&mut reader, &[b"SISMEMBER", b"s", b"m\0"], integer(1));
    assert_eq!(reader.call(&[b"WATCH", b"h", b"s"]), ok());
    assert_eq!(
        writer.call(&[b"HSETNX", b"h", b"f\0", b"ignored"]),
        integer(0)
    );
    assert_eq!(writer.call(&[b"SADD", b"s", b"m\0"]), integer(0));
    assert_eq!(writer.call(&[b"SET", b"sentinel", b"1"]), ok());
    wait_for(&mut reader, &[b"GET", b"sentinel"], bulk(b"1"));
    assert_eq!(reader.call(&[b"MULTI"]), ok());
    assert_eq!(reader.call(&[b"EXEC"]), array(vec![]));
    assert_eq!(reader.call(&[b"WATCH", b"h"]), ok());
    assert_eq!(writer.call(&[b"HSET", b"h", b"f\0", b"v\0"]), integer(0));
    assert_eq!(writer.call(&[b"SET", b"sentinel", b"2"]), ok());
    wait_for(&mut reader, &[b"GET", b"sentinel"], bulk(b"2"));
    assert_eq!(reader.call(&[b"MULTI"]), ok());
    assert_eq!(reader.call(&[b"EXEC"]), Reply::Array(None));
    for command in [
        &[b"HDEL".as_slice(), b"h", b"f\0"][..],
        &[b"SREM".as_slice(), b"s", b"m\0"][..],
    ] {
        assert_eq!(
            reader.call(command),
            Reply::Error(b"READONLY You can't write against a read only replica.".to_vec())
        );
        assert_eq!(writer.call(command), integer(1));
    }
    assert_eq!(writer.call(&[b"SET", b"sentinel", b"3"]), ok());
    wait_for(&mut reader, &[b"GET", b"sentinel"], bulk(b"3"));
    assert_eq!(reader.call(&[b"EXISTS", b"h", b"s"]), integer(0));
}

#[test]
fn snapshots_restore_hashes_sets_and_their_expiry_and_deletion() {
    let directory = common::temp_path("reru-collections", "");
    std::fs::create_dir_all(&directory).unwrap();
    let snapshot = directory.join("data.snap");
    let path = snapshot.to_str().unwrap();
    {
        let server = common::Server::start_with(&["--snapshot", path]);
        let mut c = Client::new(&server);
        assert_eq!(c.call(&[b"HSET", b"h", b"f\0", b"v\0"]), integer(1));
        assert_eq!(c.call(&[b"SADD", b"s", b"a", b"b\0"]), integer(2));
        assert_eq!(c.call(&[b"EXPIRE", b"h", b"100"]), integer(1));
        assert_eq!(c.call(&[b"HINCRBY", b"h", b"n", b"7"]), integer(7));
        assert_eq!(c.call(&[b"SREM", b"s", b"a"]), integer(1));
        assert_eq!(c.call(&[b"HSET", b"gone-h", b"f", b"v"]), integer(1));
        assert_eq!(c.call(&[b"HDEL", b"gone-h", b"f"]), integer(1));
        assert_eq!(c.call(&[b"SADD", b"gone-s", b"m"]), integer(1));
        assert_eq!(c.call(&[b"SREM", b"gone-s", b"m"]), integer(1));
    }
    {
        let server = common::Server::start_with(&["--snapshot", path]);
        let mut c = Client::new(&server);
        assert_eq!(c.call(&[b"HGET", b"h", b"f\0"]), bulk(b"v\0"));
        assert_eq!(c.call(&[b"HGET", b"h", b"n"]), bulk(b"7"));
        assert_eq!(c.call(&[b"SMEMBERS", b"s"]), members(&[b"b\0"]));
        let Reply::Integer(ttl) = c.call(&[b"TTL", b"h"]) else {
            panic!("expected TTL")
        };
        assert!((90..=100).contains(&ttl));
        assert_eq!(c.call(&[b"TTL", b"s"]), integer(-1));
        assert_eq!(c.call(&[b"EXISTS", b"gone-h", b"gone-s"]), integer(0));
    }
    std::fs::remove_file(snapshot).unwrap();
    std::fs::remove_file(directory.join("data.bak")).unwrap();
    std::fs::remove_dir(directory).unwrap();
}

#[test]
fn negative_hash_field_expiry_is_rejected_without_changes() {
    let server = common::Server::start();
    let mut c = Client::new(&server);
    assert_eq!(
        c.call(&[b"HSET", b"h", b"keep", b"v", b"live", b"w"]),
        integer(2)
    );
    for command in [
        &[b"HEXPIRE".as_slice(), b"h", b"-1", b"FIELDS", b"1", b"keep"] as &[&[u8]],
        &[b"HPEXPIRE", b"h", b"-1", b"FIELDS", b"1", b"keep"],
        &[b"HEXPIREAT", b"h", b"-1", b"FIELDS", b"1", b"keep"],
        &[b"HPEXPIREAT", b"h", b"-1", b"FIELDS", b"1", b"keep"],
        &[b"HGETEX", b"h", b"EX", b"-1", b"FIELDS", b"1", b"keep"],
        &[b"HSETEX", b"h", b"EX", b"-1", b"FIELDS", b"1", b"new", b"x"],
    ] {
        assert_eq!(
            c.call(command),
            Reply::Error(b"ERR invalid expire time, must be >= 0".to_vec())
        );
    }
    assert_eq!(
        c.call(&[b"HGETALL", b"h"]),
        array(vec![bulk(b"keep"), bulk(b"v"), bulk(b"live"), bulk(b"w"),])
    );
    assert_eq!(c.call(&[b"HGET", b"h", b"new"]), Reply::Bulk(None));
}

#[test]
fn hash_field_count_and_arity_errors_match_redis() {
    let server = common::Server::start();
    let mut c = Client::new(&server);
    assert_eq!(c.call(&[b"HSET", b"h", b"f", b"v"]), integer(1));
    let syntax = |message: &str| Reply::Error(format!("ERR {message}").into_bytes());

    assert_eq!(
        c.call(&[b"HEXPIRE", b"h", b"10", b"FOO", b"f"]),
        syntax("wrong number of arguments for 'hexpire' command")
    );
    assert_eq!(
        c.call(&[b"HEXPIRE", b"h", b"10", b"f"]),
        syntax("wrong number of arguments for 'hexpire' command")
    );
    assert_eq!(
        c.call(&[b"HEXPIRE", b"h", b"10", b"FIELDS", b"2", b"f"]),
        syntax("wrong number of arguments")
    );
    assert_eq!(
        c.call(&[b"HEXPIRE", b"h", b"10", b"FIELDS", b"1", b"f", b"g"]),
        syntax("unknown argument: g")
    );
    assert_eq!(
        c.call(&[b"HGETEX", b"h", b"FOO"]),
        syntax("wrong number of arguments for 'hgetex' command")
    );
    assert_eq!(
        c.call(&[b"HGETEX", b"h", b"FIELDS", b"2", b"f"]),
        syntax("wrong number of arguments")
    );
    assert_eq!(
        c.call(&[b"HGETEX", b"h", b"FIELDS", b"1", b"f", b"g"]),
        syntax("unknown argument: g")
    );
    assert_eq!(
        c.call(&[b"HSETEX", b"h", b"EX", b"10", b"FOO", b"f", b"v"]),
        syntax("unknown argument: FOO")
    );
    assert_eq!(
        c.call(&[b"HSETEX", b"h", b"EX", b"10", b"FIELDS", b"2", b"f", b"v"]),
        syntax("wrong number of arguments")
    );
    assert_eq!(
        c.call(&[b"HSETEX", b"h", b"EX", b"10", b"FIELDS", b"1", b"f", b"v", b"g", b"w"]),
        syntax("unknown argument: g")
    );
    assert_eq!(
        c.call(&[b"HSETEX", b"h", b"EX", b"10", b"FIELDS", b"1", b"f"]),
        syntax("wrong number of arguments")
    );
    assert_eq!(
        c.call(&[b"HSETEX", b"h", b"EX", b"10", b"FIELDS", b"1", b"f", b"v"]),
        integer(1)
    );
}

#[test]
fn hash_expiry_options_are_accepted_around_fields() {
    let server = common::Server::start();
    let mut c = Client::new(&server);
    let error = |message: &str| Reply::Error(format!("ERR {message}").into_bytes());
    assert_eq!(c.call(&[b"HSET", b"h", b"f", b"v"]), integer(1));

    // Options are accepted before or after the field list.
    assert_eq!(
        c.call(&[b"HEXPIRE", b"h", b"10", b"FIELDS", b"1", b"f", b"NX"]),
        array(vec![integer(1)])
    );
    assert_eq!(
        c.call(&[b"HEXPIRE", b"h", b"10", b"NX", b"FIELDS", b"1", b"f"]),
        array(vec![integer(0)])
    );
    assert_eq!(
        c.call(&[b"HGETEX", b"h", b"FIELDS", b"1", b"f", b"EX", b"10"]),
        array(vec![bulk(b"v")])
    );
    assert_eq!(
        c.call(&[b"HSETEX", b"h", b"FIELDS", b"1", b"f", b"v", b"EX", b"10"]),
        integer(1)
    );
    assert!(matches!(
        c.call(&[b"HTTL", b"h", b"FIELDS", b"1", b"f"]),
        Reply::Array(Some(rows)) if matches!(rows[0], Reply::Integer(ttl) if (9..=10).contains(&ttl))
    ));

    // Duplicate keywords and conflicting flags.
    assert_eq!(
        c.call(&[b"HEXPIRE", b"h", b"10", b"FIELDS", b"1", b"f", b"NX", b"XX"]),
        error("Multiple condition flags specified")
    );
    assert_eq!(
        c.call(&[b"HEXPIRE", b"h", b"10", b"FIELDS", b"1", b"f", b"FIELDS", b"1", b"g"]),
        error("FIELDS keyword specified multiple times")
    );
    assert_eq!(
        c.call(&[b"HSETEX", b"h", b"EX", b"10", b"FIELDS", b"1", b"f", b"v", b"EX", b"20"]),
        error("Only one of EX, PX, EXAT, PXAT or KEEPTTL arguments can be specified")
    );
    assert_eq!(
        c.call(&[b"HGETEX", b"h", b"FIELDS", b"1", b"f", b"PERSIST", b"EX", b"10"]),
        error("Only one of EX, PX, EXAT, PXAT or PERSIST arguments can be specified")
    );
    assert_eq!(
        c.call(&[b"HSETEX", b"h", b"FIELDS", b"1", b"f", b"v", b"FNX", b"FXX"]),
        error("Only one of FXX or FNX arguments can be specified")
    );

    // Unknown tokens keep their position-specific text, including raw bytes.
    assert_eq!(
        c.call(&[b"HEXPIRE", b"h", b"10", b"FIELDS", b"1", b"f", b"FOO"]),
        error("unknown argument: FOO")
    );
    assert_eq!(
        c.call(&[b"HGETEX", b"h", b"EX", b"10", b"f"]),
        error("unknown argument: f")
    );
    assert_eq!(
        c.call(&[b"HSETEX", b"h", b"FOO", b"FIELDS", b"1", b"f", b"v"]),
        error("unknown argument: FOO")
    );
    assert_eq!(
        c.call(&[b"HEXPIRE", b"h", b"10", b"FIELDS", b"1", b"f", b"\xff"]),
        Reply::Error(b"ERR unknown argument: \xff".to_vec())
    );

    // The TTL family uses its own field-count texts.
    assert_eq!(
        c.call(&[b"HTTL", b"h", b"FIELDS", b"1", b"f", b"g"]),
        error("The `numfields` parameter must match the number of arguments")
    );
    assert_eq!(
        c.call(&[b"HTTL", b"h", b"FIELDS", b"0", b"f"]),
        error("Number of fields must be a positive integer")
    );

    // A wrong type wins before field-list validation for HEXPIRE/HGETEX, but
    // HSETEX validates its arguments first.
    assert_eq!(c.call(&[b"RPUSH", b"list", b"x"]), integer(1));
    assert_eq!(
        c.call(&[b"HEXPIRE", b"list", b"10", b"FIELDS", b"2", b"f"]),
        Reply::Error(b"WRONGTYPE Operation against a key holding the wrong kind of value".to_vec())
    );
    assert_eq!(
        c.call(&[b"HSETEX", b"list", b"EX", b"10", b"FIELDS", b"2", b"f", b"v"]),
        error("wrong number of arguments")
    );
    assert_eq!(
        c.call(&[b"HSETEX", b"list", b"EX", b"10", b"FIELDS", b"1", b"f", b"v"]),
        Reply::Error(b"WRONGTYPE Operation against a key holding the wrong kind of value".to_vec())
    );
}
