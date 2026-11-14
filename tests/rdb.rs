// SPDX-License-Identifier: AGPL-3.0-only
#[path = "common/client.rs"]
#[allow(dead_code)]
mod client;
#[allow(dead_code)]
mod common;

use client::{array, integer, ok, queued, Client, Reply};
use common::Server;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn directory() -> PathBuf {
    let path = common::temp_path("reru-rdb", "");
    std::fs::create_dir(&path).unwrap();
    path
}

fn payload(reply: Reply) -> Vec<u8> {
    match reply {
        Reply::Bulk(Some(value)) => value,
        other => panic!("expected payload, got {other:?}"),
    }
}

#[test]
fn rdb_rejects_field_ttl_instead_of_silently_dropping_it() {
    let dir = directory();
    let server = Server::start_with(&["--dir", dir.to_str().unwrap(), "--dbfilename", "field.rdb"]);
    let mut c = Client::new(&server);
    assert_eq!(c.call(&[b"HSET", b"hash", b"field", b"value"]), integer(1));
    assert_eq!(
        c.call(&[b"HPEXPIRE", b"hash", b"100000", b"FIELDS", b"1", b"field"]),
        array(vec![integer(1)])
    );
    assert!(
        matches!(c.call(&[b"SAVE"]), Reply::Error(e) if e.windows(b"field expiry".len()).any(|part| part == b"field expiry"))
    );
    assert!(
        matches!(c.call(&[b"BGSAVE"]), Reply::Error(e) if e.windows(b"field expiry".len()).any(|part| part == b"field expiry"))
    );
    assert!(
        matches!(c.call(&[b"DUMP", b"hash"]), Reply::Error(e) if e.windows(b"field expiry".len()).any(|part| part == b"field expiry"))
    );
    assert!(!dir.join("field.rdb").exists());
    assert_eq!(
        c.call(&[b"HGET", b"hash", b"field"]),
        Reply::Bulk(Some(b"value".to_vec()))
    );
    assert_eq!(
        c.call(&[b"HPERSIST", b"hash", b"FIELDS", b"1", b"field"]),
        array(vec![integer(1)])
    );
    assert_eq!(c.call(&[b"SAVE"]), ok());
    drop(server);
    std::fs::remove_dir_all(dir).unwrap();
}

fn seed(c: &mut Client) {
    assert_eq!(c.call(&[b"SET", b"binary", b"\0\xff\r\n"]), ok());
    assert_eq!(
        c.call(&[b"RPUSH", b"list", b"1", b"\0", b"-32769"]),
        integer(3)
    );
    assert_eq!(
        c.call(&[b"HSET", b"hash", b"f", b"v", b"empty", b""]),
        integer(2)
    );
    assert_eq!(c.call(&[b"SADD", b"set", b"a", b"b"]), integer(2));
    assert_eq!(
        c.call(&[b"ZADD", b"zset", b"1.25", b"a", b"inf", b"b"]),
        integer(2)
    );
}

fn verify(c: &mut Client) {
    assert_eq!(payload(c.call(&[b"GET", b"binary"])), b"\0\xff\r\n");
    assert_eq!(c.call(&[b"LLEN", b"list"]), integer(3));
    assert_eq!(payload(c.call(&[b"LINDEX", b"list", b"2"])), b"-32769");
    assert_eq!(payload(c.call(&[b"HGET", b"hash", b"empty"])), b"");
    assert_eq!(c.call(&[b"SCARD", b"set"]), integer(2));
    assert_eq!(payload(c.call(&[b"ZSCORE", b"zset", b"b"])), b"inf");
}

#[test]
fn dump_restore_all_supported_types_and_validate_payload_before_replacing() {
    let server = Server::start();
    let mut c = Client::new(&server);
    seed(&mut c);
    for key in [b"binary".as_slice(), b"list", b"hash", b"set", b"zset"] {
        let bytes = payload(c.call(&[b"DUMP", key]));
        assert!(
            matches!(c.call(&[b"RESTORE", key, b"0", &bytes]), Reply::Error(e) if e.starts_with(b"BUSYKEY"))
        );
        assert_eq!(c.call(&[b"RESTORE", key, b"0", &bytes, b"REPLACE"]), ok());
        let mut corrupt = bytes.clone();
        corrupt[1] ^= 1;
        assert!(matches!(
            c.call(&[b"RESTORE", key, b"0", &corrupt, b"REPLACE"]),
            Reply::Error(_)
        ));
        assert_eq!(payload(c.call(&[b"DUMP", key])), bytes);
    }
    verify(&mut c);
    assert_eq!(c.call(&[b"DUMP", b"missing"]), Reply::Bulk(None));
    assert!(matches!(
        c.call(&[b"RESTORE", b"new", b"-1", b"invalid"]),
        Reply::Error(_)
    ));
    assert!(matches!(
        c.call(&[b"RESTORE", b"new", b"0", b"invalid", b"unknown"]),
        Reply::Error(_)
    ));
}

#[test]
fn restore_expiry_watch_transactions_and_memory_rejection() {
    let server = Server::start();
    let mut c = Client::new(&server);
    let mut watcher = Client::new(&server);
    c.call(&[b"SET", b"source", b"value"]);
    let bytes = payload(c.call(&[b"DUMP", b"source"]));
    assert_eq!(watcher.call(&[b"WATCH", b"new"]), ok());
    assert_eq!(c.call(&[b"MULTI"]), ok());
    assert_eq!(c.call(&[b"RESTORE", b"new", b"60000", &bytes]), queued());
    assert_eq!(c.call(&[b"EXEC"]), array(vec![ok()]));
    assert!(matches!(c.call(&[b"PTTL", b"new"]), Reply::Integer(n) if n > 0 && n <= 60000));
    watcher.call(&[b"MULTI"]);
    watcher.call(&[b"GET", b"new"]);
    assert_eq!(watcher.call(&[b"EXEC"]), Reply::Array(None));
    assert_eq!(
        c.call(&[b"RESTORE", b"new", b"1", &bytes, b"REPLACE", b"ABSTTL"]),
        ok()
    );
    assert_eq!(c.call(&[b"EXISTS", b"new"]), integer(0));
    let deadline = (SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis()
        + 60000)
        .to_string();
    assert_eq!(
        c.call(&[b"RESTORE", b"new", deadline.as_bytes(), &bytes, b"ABSTTL"]),
        ok()
    );
    assert!(matches!(c.call(&[b"PTTL", b"new"]), Reply::Integer(n) if n > 0 && n <= 60000));
    let bounded = Server::start_with(&["--maxmemory", "1"]);
    let mut limited = Client::new(&bounded);
    assert!(
        matches!(limited.call(&[b"RESTORE", b"key", b"0", &bytes]), Reply::Error(e) if e.starts_with(b"OOM"))
    );
    assert_eq!(limited.call(&[b"EXISTS", b"key"]), integer(0));
}

#[test]
fn save_and_background_save_survive_restart_with_expiry() {
    let dir = directory();
    let path = dir.to_str().unwrap();
    {
        let server = Server::start_with(&["--dir", path, "--dbfilename", "test.rdb"]);
        let mut c = Client::new(&server);
        seed(&mut c);
        assert_eq!(c.call(&[b"SET", b"expires", b"yes", b"PX", b"60000"]), ok());
        assert_eq!(c.call(&[b"SAVE"]), ok());
        assert!(std::fs::read(dir.join("test.rdb"))
            .unwrap()
            .starts_with(b"REDIS0010"));
        assert!(matches!(c.call(&[b"LASTSAVE"]), Reply::Integer(n) if n > 0));
        assert_eq!(
            c.call(&[b"CONFIG", b"GET", b"dbfilename"]),
            array(vec![
                Reply::Bulk(Some(b"dbfilename".to_vec())),
                Reply::Bulk(Some(b"test.rdb".to_vec()))
            ])
        );
        c.call(&[b"SET", b"background", b"saved"]);
        assert_eq!(
            c.call(&[b"BGSAVE"]),
            Reply::Simple(b"Background saving started".to_vec())
        );
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let info = payload(c.call(&[b"INFO", b"persistence"]));
            if String::from_utf8_lossy(&info).contains("rdb_bgsave_in_progress:0") {
                assert!(String::from_utf8_lossy(&info).contains("rdb_last_bgsave_status:ok"));
                break;
            }
            assert!(Instant::now() < deadline, "background save timeout");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    {
        let server = Server::start_with(&["--dir", path, "--dbfilename", "test.rdb"]);
        let mut c = Client::new(&server);
        verify(&mut c);
        assert_eq!(payload(c.call(&[b"GET", b"background"])), b"saved");
        assert!(matches!(c.call(&[b"PTTL", b"expires"]), Reply::Integer(n) if n > 0 && n < 60000));
        let before = std::fs::read(dir.join("test.rdb")).unwrap();
        c.call(&[b"XADD", b"stream", b"1-0", b"f", b"v"]);
        assert!(matches!(c.call(&[b"SAVE"]), Reply::Error(_)));
        assert_eq!(std::fs::read(dir.join("test.rdb")).unwrap(), before);
        assert!(matches!(c.call(&[b"DUMP", b"stream"]), Reply::Error(_)));
        c.call(&[b"MULTI"]);
        assert!(matches!(c.call(&[b"SAVE"]), Reply::Error(_)));
        assert!(matches!(c.call(&[b"EXEC"]), Reply::Error(e) if e.starts_with(b"EXECABORT")));
    }
    std::fs::remove_file(dir.join("test.rdb")).unwrap();
    std::fs::remove_dir(dir).unwrap();
}

#[test]
fn restored_data_survives_aof_replay() {
    let source = Server::start();
    let mut c = Client::new(&source);
    seed(&mut c);
    let bytes = payload(c.call(&[b"DUMP", b"zset"]));
    let dir = directory();
    let aof = dir.join("test.aof");
    {
        let server = Server::start_with(&["--appendonly", aof.to_str().unwrap()]);
        let mut c = Client::new(&server);
        assert_eq!(c.call(&[b"RESTORE", b"z", b"0", &bytes]), ok());
    }
    {
        let server = Server::start_with(&["--appendonly", aof.to_str().unwrap()]);
        let mut c = Client::new(&server);
        assert_eq!(payload(c.call(&[b"ZSCORE", b"z", b"a"])), b"1.25");
    }
    std::fs::remove_file(aof).unwrap();
    std::fs::remove_dir(dir).unwrap();
}

#[test]
fn redis_dump_restore_interoperability() {
    let Ok(port) = std::env::var("RERU_REDIS_REFERENCE_PORT") else {
        return;
    };
    let mut redis = Client::on_port(port.parse().unwrap());
    let server = Server::start();
    let mut ours = Client::new(&server);
    let key = format!("reru-rdb-test:{}", std::process::id());
    let key = key.as_bytes();
    let compressed = vec![b'x'; 20000];
    let commands: Vec<Vec<&[u8]>> = vec![
        vec![b"SET", key, b"\0\xff"],
        vec![b"SET", key, b"-120"],
        vec![b"SET", key, b"-30000"],
        vec![b"SET", key, b"-2000000000"],
        vec![b"SET", key, &compressed],
        vec![
            b"RPUSH",
            key,
            b"127",
            b"-4096",
            b"4095",
            b"32767",
            b"-32768",
            b"8388607",
            b"-8388608",
            b"2147483647",
            b"-9223372036854775808",
            b"hello\0",
        ],
        vec![b"SADD", key, b"-32769", b"1", b"9223372036854775807"],
        vec![b"SADD", key, b"a", b"b"],
        vec![b"HSET", key, b"field", b"value", b"number", b"42"],
        vec![b"ZADD", key, b"1.25", b"a", b"-inf", b"b"],
    ];
    for command in commands {
        redis.call(&[b"DEL", key]);
        ours.call(&[b"DEL", key]);
        assert_eq!(redis.call(&command), ours.call(&command));
        let from_redis = payload(redis.call(&[b"DUMP", key]));
        assert_eq!(
            ours.call(&[b"RESTORE", key, b"0", &from_redis, b"REPLACE"]),
            ok(),
            "Redis payload: {from_redis:?}"
        );
        let from_ours = payload(ours.call(&[b"DUMP", key]));
        assert_eq!(
            redis.call(&[b"RESTORE", key, b"0", &from_ours, b"REPLACE"]),
            ok()
        );
        let query: Vec<&[u8]> = match command[0] {
            b"SET" => vec![b"GET", key],
            b"RPUSH" => vec![b"LRANGE", key, b"0", b"-1"],
            b"SADD" => vec![b"SMISMEMBER", key, b"1", b"a", b"9223372036854775807"],
            b"HSET" => vec![b"HMGET", key, b"field", b"number"],
            _ => vec![b"ZRANGE", key, b"0", b"-1", b"WITHSCORES"],
        };
        assert_eq!(ours.call(&query), redis.call(&query));
    }
    redis.call(&[b"DEL", key]);
}
