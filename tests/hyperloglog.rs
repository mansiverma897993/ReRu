// SPDX-License-Identifier: AGPL-3.0-only
#[path = "common/client.rs"]
#[allow(dead_code)]
mod client;
#[allow(dead_code)]
mod common;
use client::{array, integer, ok, Client, Reply};
use common::Server;
use std::time::{Duration, Instant};

fn bulk(value: &[u8]) -> Reply {
    Reply::Bulk(Some(value.to_vec()))
}
fn bytes(reply: Reply) -> Vec<u8> {
    let Reply::Bulk(Some(bytes)) = reply else {
        panic!("expected bytes: {reply:?}")
    };
    bytes
}
fn wait(mut condition: impl FnMut() -> bool) {
    let until = Instant::now() + Duration::from_secs(5);
    while !condition() {
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn path() -> std::path::PathBuf {
    common::temp_path("reru-hll", "")
}

#[test]
fn binary_members_idempotence_union_merge_aliases_and_ttl() {
    let server = Server::start();
    let mut c = Client::new(&server);
    assert_eq!(c.call(&[b"PFCOUNT", b"absent"]), integer(0));
    assert_eq!(c.call(&[b"EXISTS", b"absent"]), integer(0));
    assert_eq!(c.call(&[b"PFADD", b"h"]), integer(1));
    assert_eq!(c.call(&[b"PFADD", b"h"]), integer(0));
    assert_eq!(c.call(&[b"TYPE", b"h"]), Reply::Simple(b"string".to_vec()));
    assert_eq!(
        c.call(&[b"PFADD", b"h", b"", b"a\0\xff", b"\r\n", b"x"]),
        integer(1)
    );
    assert_eq!(c.call(&[b"PFCOUNT", b"h"]), integer(4));
    let raw = c.call(&[b"GET", b"h"]);
    assert_eq!(c.call(&[b"PFADD", b"h", b"x", b"", b"x"]), integer(0));
    assert_eq!(c.call(&[b"GET", b"h"]), raw);
    c.call(&[b"PEXPIRE", b"h", b"60000"]);
    let deadline = c.call(&[b"PEXPIRETIME", b"h"]);
    c.call(&[b"PFADD", b"other", b"x", b"new"]);
    assert_eq!(
        c.call(&[b"PFCOUNT", b"h", b"other", b"absent", b"h"]),
        integer(5)
    );
    assert_eq!(c.call(&[b"PFMERGE", b"h", b"other", b"h", b"absent"]), ok());
    assert_eq!(c.call(&[b"PEXPIRETIME", b"h"]), deadline);
    assert_eq!(c.call(&[b"PFCOUNT", b"h"]), integer(5));
    assert_eq!(c.call(&[b"PFMERGE", b"empty"]), ok());
    assert_eq!(c.call(&[b"PFCOUNT", b"empty"]), integer(0));
    c.call(&[b"SET", b"copy", &bytes(raw)]);
    assert_eq!(c.call(&[b"PFCOUNT", b"copy"]), integer(4));
    c.call(&[b"PEXPIREAT", b"h", b"1"]);
    assert_eq!(c.call(&[b"PFCOUNT", b"h"]), integer(0));
    assert_eq!(c.call(&[b"PFADD", b"h", b"fresh"]), integer(1));
    assert_eq!(c.call(&[b"PTTL", b"h"]), integer(-1));
    for name in [b"PFADD".as_slice(), b"PFCOUNT", b"PFMERGE"] {
        assert!(matches!(c.call(&[name]), Reply::Error(_)));
    }
}

#[test]
fn invalid_inputs_do_not_change_destination_or_invalidate_watches() {
    let server = Server::start();
    let mut c = Client::new(&server);
    let mut watcher = Client::new(&server);
    c.call(&[b"PFADD", b"destination", b"a"]);
    c.call(&[b"PFCOUNT", b"destination"]);
    c.call(&[b"SET", b"invalid", b"plain"]);
    c.call(&[b"RPUSH", b"list", b"v"]);
    let mut corrupt = b"HYLL\x01\0\0\0\0\0\0\0\0\0\0\x80".to_vec();
    corrupt.push(0x40); // Truncated XZERO opcode.
    c.call(&[b"SET", b"corrupt", &corrupt]);
    let before = c.call(&[b"GET", b"destination"]);
    watcher.call(&[b"WATCH", b"destination"]);
    for key in [b"invalid".as_slice(), b"list", b"corrupt"] {
        for command in [
            vec![b"PFCOUNT".as_slice(), key],
            vec![b"PFADD".as_slice(), key, b"x"],
            vec![b"PFMERGE".as_slice(), b"destination", key],
        ] {
            assert!(matches!(c.call(&command), Reply::Error(_)));
        }
    }
    assert_eq!(c.call(&[b"GET", b"destination"]), before);
    watcher.call(&[b"MULTI"]);
    watcher.call(&[b"PFCOUNT", b"destination"]);
    assert_eq!(watcher.call(&[b"EXEC"]), array(vec![integer(1)]));
    assert_eq!(c.call(&[b"GET", b"corrupt"]), bulk(&corrupt));
}

#[test]
fn count_cache_watch_transactions_and_oom_rollback() {
    let server = Server::start();
    let mut c = Client::new(&server);
    let mut watcher = Client::new(&server);
    c.call(&[b"PFADD", b"h", b"a"]);
    watcher.call(&[b"WATCH", b"h"]);
    assert_eq!(c.call(&[b"PFCOUNT", b"h"]), integer(1));
    watcher.call(&[b"MULTI"]);
    watcher.call(&[b"GET", b"h"]);
    assert_eq!(watcher.call(&[b"EXEC"]), Reply::Array(None));
    watcher.call(&[b"WATCH", b"h"]);
    c.call(&[b"PFCOUNT", b"h"]);
    c.call(&[b"PFADD", b"h", b"a"]);
    watcher.call(&[b"MULTI"]);
    watcher.call(&[b"PFCOUNT", b"h"]);
    assert_eq!(watcher.call(&[b"EXEC"]), array(vec![integer(1)]));
    c.call(&[b"MULTI"]);
    c.call(&[b"PFADD", b"h", b"b"]);
    c.call(&[b"PFCOUNT", b"h"]);
    c.call(&[b"PFMERGE", b"merged", b"h"]);
    assert_eq!(
        c.call(&[b"EXEC"]),
        array(vec![integer(1), integer(2), ok()])
    );
    c.call(&[
        b"CONFIG",
        b"SET",
        b"maxmemory",
        b"1",
        b"maxmemory-policy",
        b"noeviction",
    ]);
    watcher.call(&[b"WATCH", b"missing"]);
    assert!(
        matches!(c.call(&[b"PFADD", b"missing", b"a"]), Reply::Error(e) if e.starts_with(b"OOM"))
    );
    assert_eq!(c.call(&[b"EXISTS", b"missing"]), integer(0));
    watcher.call(&[b"MULTI"]);
    watcher.call(&[b"EXISTS", b"missing"]);
    assert_eq!(watcher.call(&[b"EXEC"]), array(vec![integer(0)]));
    // Cached cardinality is a read even when the dataset exceeds maxmemory.
    assert_eq!(c.call(&[b"PFCOUNT", b"h"]), integer(2));
    c.call(&[b"CONFIG", b"SET", b"maxmemory", b"0"]);
    let sparse = b"HYLL\x01\0\0\0\0\0\0\0\0\0\0\x80\x7f\xff";
    c.call(&[b"SET", b"sparse", sparse]);
    watcher.call(&[b"WATCH", b"sparse"]);
    c.call(&[b"CONFIG", b"SET", b"maxmemory", b"1"]);
    assert!(
        matches!(c.call(&[b"PFADD", b"sparse", b"new"]), Reply::Error(e) if e.starts_with(b"OOM"))
    );
    assert!(
        matches!(c.call(&[b"PFMERGE", b"sparse", b"h"]), Reply::Error(e) if e.starts_with(b"OOM"))
    );
    assert_eq!(c.call(&[b"GET", b"sparse"]), bulk(sparse));
    // Multi-key counting does not update either source's cached header.
    assert_eq!(c.call(&[b"PFCOUNT", b"sparse", b"h"]), integer(2));
    assert_eq!(c.call(&[b"GET", b"sparse"]), bulk(sparse));
    watcher.call(&[b"MULTI"]);
    watcher.call(&[b"GET", b"sparse"]);
    assert_eq!(watcher.call(&[b"EXEC"]), array(vec![bulk(sparse)]));
}

#[test]
fn hyperloglog_survives_snapshot_aof_rewrite_rdb_and_dump_restore() {
    for mode in ["--snapshot", "--appendonly", "rdb"] {
        let path = path();
        std::fs::create_dir(&path).unwrap();
        let file = path.join("data");
        let args = if mode == "rdb" {
            vec!["--dir", path.to_str().unwrap(), "--dbfilename", "data"]
        } else {
            vec![mode, file.to_str().unwrap()]
        };
        let cached;
        let expiry;
        {
            let server = Server::start_with(&args);
            let mut c = Client::new(&server);
            c.call(&[b"PFADD", b"h", b"a", b"b", b"\0\xff"]);
            c.call(&[b"PEXPIRE", b"h", b"60000"]);
            expiry = c.call(&[b"PEXPIRETIME", b"h"]);
            // Count is the final state-changing command; persist its cache too.
            c.call(&[b"MULTI"]);
            c.call(&[b"PFCOUNT", b"h"]);
            assert_eq!(c.call(&[b"EXEC"]), array(vec![integer(3)]));
            cached = c.call(&[b"GET", b"h"]);
            if mode == "rdb" {
                assert_eq!(c.call(&[b"SAVE"]), ok());
            }
            if mode == "--appendonly" {
                c.call(&[b"BGREWRITEAOF"]);
                wait(
                    || matches!(c.call(&[b"INFO", b"persistence"]), Reply::Bulk(Some(bytes)) if String::from_utf8_lossy(&bytes).contains("aof_rewrites:1\r\n")),
                );
            }
        }
        {
            let server = Server::start_with(&args);
            let mut c = Client::new(&server);
            assert_eq!(c.call(&[b"GET", b"h"]), cached);
            let Reply::Integer(before) = expiry else {
                panic!()
            };
            let Reply::Integer(after) = c.call(&[b"PEXPIRETIME", b"h"]) else {
                panic!()
            };
            // Snapshot/AOF deadlines cross Instant/SystemTime with millisecond
            // rounding; require the same deadline within that precision.
            assert!((after - before).abs() <= 2, "{mode}: {before} -> {after}");
            assert_eq!(c.call(&[b"PFCOUNT", b"h"]), integer(3));
            let dump = bytes(c.call(&[b"DUMP", b"h"]));
            assert_eq!(c.call(&[b"RESTORE", b"restored", b"0", &dump]), ok());
            assert_eq!(c.call(&[b"PFCOUNT", b"restored"]), integer(3));
        }
        std::fs::remove_file(&file).unwrap();
        std::fs::remove_file(file.with_extension("bak")).ok();
        std::fs::remove_dir(&path).unwrap();
    }
}

#[test]
fn replica_receives_hll_updates_and_allows_counts_but_rejects_writes() {
    let primary = Server::start();
    let mut p = Client::new(&primary);
    p.call(&[b"PFADD", b"h", b"a", b"b"]);
    let replica = Server::start_with(&["--replicaof", "127.0.0.1", &primary.port.to_string()]);
    let mut r = Client::new(&replica);
    wait(|| r.call(&[b"EXISTS", b"h"]) == integer(1));
    assert_eq!(r.call(&[b"PFCOUNT", b"h"]), integer(2));
    for command in [
        vec![b"PFADD".as_slice(), b"h", b"c"],
        vec![b"PFMERGE".as_slice(), b"h", b"h"],
    ] {
        assert!(matches!(r.call(&command), Reply::Error(e) if e.starts_with(b"READONLY")));
    }
    p.call(&[b"PFADD", b"h", b"c"]);
    wait(|| r.call(&[b"PFCOUNT", b"h"]) == integer(3));
    p.call(&[b"PFCOUNT", b"h"]);
    let raw = p.call(&[b"GET", b"h"]);
    wait(|| r.call(&[b"GET", b"h"]) == raw);
}

#[test]
fn redis_reference_matches_hashes_estimates_union_sparse_and_dense_exchange() {
    let Ok(port) = std::env::var("RERU_REDIS_REFERENCE_PORT") else {
        return;
    };
    let server = Server::start();
    let mut ours = Client::new(&server);
    let mut redis = Client::on_port(port.parse().unwrap());
    let keys = [
        b"reru-diff:hll-a".as_slice(),
        b"reru-diff:hll-b",
        b"reru-diff:hll-merged",
        b"reru-diff:hll-copy",
    ];
    for key in keys {
        ours.call(&[b"DEL", key]);
        redis.call(&[b"DEL", key]);
    }
    for command in [
        vec![b"PFADD".as_slice(), keys[0]],
        vec![b"PFCOUNT".as_slice(), keys[0]],
        vec![b"PFADD".as_slice(), keys[0], b"", b"a\0\xff", b"\r\n"],
        vec![b"PFCOUNT".as_slice(), keys[0]],
    ] {
        assert_eq!(ours.call(&command), redis.call(&command), "{command:?}");
    }
    let sparse = bytes(redis.call(&[b"GET", keys[0]]));
    assert_eq!(sparse[4], 1);
    ours.call(&[b"SET", keys[3], &sparse]);
    assert_eq!(ours.call(&[b"PFCOUNT", keys[3]]), integer(3));
    for client in [&mut ours, &mut redis] {
        client.call(&[b"SET", keys[3], &sparse]);
    }
    assert_eq!(
        ours.call(&[b"PFADD", keys[3], b"added-after-import"]),
        redis.call(&[b"PFADD", keys[3], b"added-after-import"])
    );
    for batch in 0..80 {
        let values: Vec<Vec<u8>> = (0..500)
            .map(|n| {
                let id = batch * 500 + n;
                let mut bytes = format!("element:{id}:").into_bytes();
                bytes.extend((0..id % 19).map(|x| (x * 37) as u8));
                bytes
            })
            .collect();
        let key = keys[batch % 2];
        let mut command = vec![b"PFADD".as_slice(), key];
        command.extend(values.iter().map(Vec::as_slice));
        assert_eq!(ours.call(&command), redis.call(&command));
        if batch % 10 == 0 {
            for key in &keys[..2] {
                assert_eq!(
                    ours.call(&[b"PFCOUNT", key]),
                    redis.call(&[b"PFCOUNT", key])
                );
            }
        }
    }
    for key in &keys[..2] {
        assert_eq!(
            ours.call(&[b"PFCOUNT", key]),
            redis.call(&[b"PFCOUNT", key])
        );
        let reference = bytes(redis.call(&[b"GET", key]));
        assert_eq!(reference[4], 0);
        assert_eq!(ours.call(&[b"GET", key]), bulk(&reference));
    }
    for command in [
        vec![b"PFCOUNT".as_slice(), keys[0], keys[1], keys[0]],
        vec![b"PFMERGE".as_slice(), keys[2], keys[0], keys[1]],
        vec![b"PFCOUNT".as_slice(), keys[2]],
    ] {
        assert_eq!(ours.call(&command), redis.call(&command), "{command:?}");
    }
    assert_eq!(
        ours.call(&[b"GET", keys[2]]),
        redis.call(&[b"GET", keys[2]])
    );
    let ours_dump = bytes(ours.call(&[b"DUMP", keys[2]]));
    assert_eq!(
        redis.call(&[b"RESTORE", keys[3], b"0", &ours_dump, b"REPLACE"]),
        ok()
    );
    assert_eq!(
        ours.call(&[b"PFCOUNT", keys[2]]),
        redis.call(&[b"PFCOUNT", keys[3]])
    );
    let reference_dump = bytes(redis.call(&[b"DUMP", keys[2]]));
    assert_eq!(
        ours.call(&[b"RESTORE", keys[3], b"0", &reference_dump, b"REPLACE"]),
        ok()
    );
    assert_eq!(
        ours.call(&[b"PFCOUNT", keys[3]]),
        redis.call(&[b"PFCOUNT", keys[2]])
    );
    for key in keys {
        redis.call(&[b"DEL", key]);
    }
}
