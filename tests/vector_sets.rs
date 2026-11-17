// SPDX-License-Identifier: AGPL-3.0-only
#[path = "common/client.rs"]
mod client;
mod common;

use client::{array, integer, ok, queued, Client, Reply};
use std::time::{Duration, Instant};

fn bulk(value: &[u8]) -> Reply {
    Reply::Bulk(Some(value.to_vec()))
}

fn error(value: &[u8]) -> Reply {
    Reply::Error(value.to_vec())
}

#[test]
fn vector_create_query_update_remove_and_errors_match_redis() {
    let server = common::Server::start();
    let mut c = Client::new(&server);
    assert_eq!(c.call(&[b"VCARD", b"missing"]), integer(0));
    assert_eq!(
        c.call(&[b"VDIM", b"missing"]),
        error(b"ERR key does not exist")
    );
    assert_eq!(c.call(&[b"VEMB", b"missing", b"a"]), Reply::Array(None));
    assert_eq!(c.call(&[b"VREM", b"missing", b"a"]), integer(0));
    assert_eq!(
        c.call(&[b"VSIM", b"missing", b"ELE", b"a", b"TRUTH"]),
        array(vec![])
    );
    assert_eq!(
        c.call(&[b"VADD", b"v", b"VALUES", b"2", b"1", b"0", b"a"]),
        integer(1)
    );
    assert_eq!(
        c.call(&[b"TYPE", b"v"]),
        Reply::Simple(b"vectorset".to_vec())
    );
    assert_eq!(c.call(&[b"VCARD", b"v"]), integer(1));
    assert_eq!(c.call(&[b"VDIM", b"v"]), integer(2));
    assert_eq!(
        c.call(&[b"VEMB", b"v", b"a"]),
        array(vec![bulk(b"1"), bulk(b"0")])
    );
    assert_eq!(
        c.call(&[b"VADD", b"v", b"VALUES", b"2", b"0", b"1", b"b"]),
        integer(1)
    );
    assert_eq!(
        c.call(&[b"VADD", b"v", b"VALUES", b"2", b"-1", b"0", b"c"]),
        integer(1)
    );
    assert_eq!(
        c.call(&[b"VADD", b"v", b"VALUES", b"2", b"1", b"0", b"a"]),
        integer(0)
    );
    assert_eq!(
        c.call(&[b"VSIM", b"v", b"ELE", b"a", b"WITHSCORES", b"TRUTH"]),
        array(vec![
            bulk(b"a"),
            bulk(b"1"),
            bulk(b"b"),
            bulk(b"0.5"),
            bulk(b"c"),
            bulk(b"0")
        ])
    );
    assert_eq!(
        c.call(&[b"VSIM", b"v", b"ELE", b"absent", b"TRUTH"]),
        error(b"ERR element not found in set")
    );
    assert_eq!(
        c.call(&[b"VADD", b"v", b"VALUES", b"3", b"1", b"0", b"0", b"d"]),
        error(b"ERR Vector dimension mismatch - got 3 but set has 2")
    );
    assert_eq!(c.call(&[b"VCARD", b"v"]), integer(3));
    assert_eq!(
        c.call(&[b"VADD", b"v", b"VALUES", b"2", b"nan", b"0", b"d"]),
        error(b"ERR invalid vector specification")
    );
    assert_eq!(c.call(&[b"VREM", b"v", b"a"]), integer(1));
    assert_eq!(c.call(&[b"VREM", b"v", b"a"]), integer(0));
    assert_eq!(c.call(&[b"SET", b"s", b"x"]), ok());
    let commands: &[&[&[u8]]] = &[
        &[b"VCARD".as_slice(), b"s"],
        &[b"VDIM", b"s"],
        &[b"VEMB", b"s", b"a"],
        &[b"VREM", b"s", b"a"],
        &[b"VSIM", b"s", b"ELE", b"a", b"TRUTH"],
        &[b"VADD", b"s", b"VALUES", b"2", b"1", b"0", b"a"],
    ];
    for cmd in commands {
        assert_eq!(
            c.call(cmd),
            error(b"WRONGTYPE Operation against a key holding the wrong kind of value")
        );
    }
}

#[test]
fn binary_fp32_and_rejected_options_are_explicit() {
    let server = common::Server::start();
    let mut c = Client::new(&server);
    let fp32 = [1f32.to_le_bytes(), 0f32.to_le_bytes()].concat();
    assert_eq!(
        c.call(&[b"VADD", b"raw", b"FP32", &fp32, b"a\0", b"NOQUANT"]),
        integer(1)
    );
    assert_eq!(
        c.call(&[b"VEMB", b"raw", b"a\0"]),
        array(vec![bulk(b"1"), bulk(b"0")])
    );
    assert_eq!(
        c.call(&[b"VEMB", b"raw", b"a\0", b"RAW"]),
        array(vec![
            Reply::Simple(b"f32".to_vec()),
            bulk(&fp32),
            bulk(b"1")
        ])
    );
    assert_eq!(
        c.call(&[b"VADD", b"raw", b"FP32", b"bad", b"b", b"NOQUANT"]),
        error(b"ERR invalid vector specification")
    );
    assert_eq!(
        c.call(&[b"VADD", b"raw", b"VALUES", b"2", b"0", b"1", b"b"]),
        error(b"ERR asked quantization mismatch with existing vector set")
    );
    assert_eq!(
        c.call(&[b"VSIM", b"raw", b"FP32", &fp32, b"WITHSCORES", b"TRUTH"]),
        array(vec![bulk(b"a\0"), bulk(b"1")])
    );
    assert_eq!(
        c.call(&[b"VSIM", b"raw", b"ELE", b"a\0"]),
        error(b"ERR approximate vector search is not supported; use TRUTH")
    );
    assert_eq!(
        c.call(&[b"VSIM", b"raw", b"ELE", b"a\0", b"FILTER", b"x", b"TRUTH"]),
        error(b"ERR unsupported VSIM option")
    );
    assert_eq!(
        c.call(&[b"VADD", b"raw", b"VALUES", b"2", b"0", b"1", b"b", b"SETATTR", b"{}"]),
        error(b"ERR unsupported VADD option")
    );
    assert_eq!(c.call(&[b"VCARD", b"raw"]), integer(1));
}

#[test]
fn noquant_truth_search_scores_and_raw_q8_encoding_match_reference() {
    let server = common::Server::start();
    let mut c = Client::new(&server);
    assert_eq!(
        c.call(&[b"VADD", b"n", b"VALUES", b"2", b"1", b"1", b"a", b"NOQUANT"]),
        integer(1)
    );
    assert_eq!(
        c.call(&[b"VADD", b"n", b"VALUES", b"2", b"0", b"1", b"b", b"NOQUANT"]),
        integer(1)
    );
    assert_eq!(
        c.call(&[b"VSIM", b"n", b"ELE", b"a", b"WITHSCORES", b"TRUTH"]),
        array(vec![
            bulk(b"a"),
            bulk(b"1"),
            bulk(b"b"),
            bulk(b"0.8535534143447876")
        ])
    );
    assert_eq!(
        c.call(&[b"VADD", b"q", b"VALUES", b"2", b"1", b"0", b"a"]),
        integer(1)
    );
    assert_eq!(
        c.call(&[b"VEMB", b"q", b"a", b"RAW"]),
        array(vec![
            Reply::Simple(b"int8".to_vec()),
            bulk(&[127, 0]),
            bulk(b"1"),
            bulk(b"1")
        ])
    );
    assert_eq!(c.call(&[b"VREM", b"n", b"a"]), integer(1));
    assert_eq!(c.call(&[b"VREM", b"n", b"b"]), integer(1));
    assert_eq!(c.call(&[b"TYPE", b"n"]), Reply::Simple(b"none".to_vec()));
}

#[test]
fn vectors_participate_in_copy_ttl_watch_and_transactions() {
    let server = common::Server::start();
    let mut c = Client::new(&server);
    let mut watcher = Client::new(&server);
    assert_eq!(
        c.call(&[b"VADD", b"v", b"VALUES", b"2", b"1", b"0", b"a"]),
        integer(1)
    );
    assert_eq!(c.call(&[b"PEXPIRE", b"v", b"30000"]), integer(1));
    assert_eq!(c.call(&[b"COPY", b"v", b"copy"]), integer(1));
    assert_eq!(
        c.call(&[b"TYPE", b"copy"]),
        Reply::Simple(b"vectorset".to_vec())
    );
    assert_eq!(
        c.call(&[b"VEMB", b"copy", b"a"]),
        array(vec![bulk(b"1"), bulk(b"0")])
    );
    assert_eq!(watcher.call(&[b"WATCH", b"v"]), ok());
    assert_eq!(
        c.call(&[b"VADD", b"v", b"VALUES", b"2", b"0", b"1", b"b"]),
        integer(1)
    );
    assert_eq!(watcher.call(&[b"MULTI"]), ok());
    assert_eq!(watcher.call(&[b"VCARD", b"v"]), queued());
    assert_eq!(watcher.call(&[b"EXEC"]), Reply::Array(None));
    assert_eq!(c.call(&[b"MULTI"]), ok());
    assert_eq!(c.call(&[b"VREM", b"v", b"b"]), queued());
    assert_eq!(c.call(&[b"VCARD", b"v"]), queued());
    assert_eq!(c.call(&[b"EXEC"]), array(vec![integer(1), integer(1)]));
    let Reply::Integer(ttl) = c.call(&[b"PTTL", b"v"]) else {
        panic!("expected TTL")
    };
    assert!((0..=30000).contains(&ttl));
    assert_eq!(c.call(&[b"DEL", b"copy"]), integer(1));
    assert_eq!(c.call(&[b"PEXPIRE", b"v", b"0"]), integer(1));
    assert_eq!(c.call(&[b"TYPE", b"v"]), Reply::Simple(b"none".to_vec()));
}

#[test]
fn noeviction_rolls_back_vector_growth_and_watch_state() {
    let server =
        common::Server::start_with(&["--maxmemory", "240", "--maxmemory-policy", "noeviction"]);
    let mut c = Client::new(&server);
    let mut watcher = Client::new(&server);
    assert_eq!(
        c.call(&[b"VADD", b"v", b"VALUES", b"2", b"1", b"0", b"a"]),
        integer(1)
    );
    assert_eq!(watcher.call(&[b"WATCH", b"v"]), ok());
    let huge = vec![b'x'; 180];
    assert!(
        matches!(c.call(&[b"VADD", b"v", b"VALUES", b"2", b"0", b"1", &huge]), Reply::Error(e) if e.starts_with(b"OOM "))
    );
    assert_eq!(c.call(&[b"VCARD", b"v"]), integer(1));
    assert_eq!(watcher.call(&[b"MULTI"]), ok());
    assert_eq!(watcher.call(&[b"VCARD", b"v"]), queued());
    assert_eq!(watcher.call(&[b"EXEC"]), array(vec![integer(1)]));
}

#[test]
fn vectors_survive_snapshot_aof_rewrite_and_replication() {
    for mode in ["--snapshot", "--appendonly"] {
        let path = common::temp_path(
            "reru-vector",
            if mode == "--snapshot" {
                ".snap"
            } else {
                ".aof"
            },
        );
        let filename = path.to_str().unwrap();
        {
            let primary = common::Server::start_with(&[mode, filename]);
            let mut c = Client::new(&primary);
            assert_eq!(
                c.call(&[b"VADD", b"v", b"VALUES", b"2", b"1", b"0", b"a\0"]),
                integer(1)
            );
            assert_eq!(
                c.call(&[b"VADD", b"v", b"VALUES", b"2", b"0", b"1", b"b"]),
                integer(1)
            );
            assert_eq!(c.call(&[b"PEXPIRE", b"v", b"30000"]), integer(1));
            assert_eq!(
                c.call(&[b"VADD", b"precise", b"VALUES", b"2", b"1", b"0", b"a", b"NOQUANT"]),
                integer(1)
            );
            let port = primary.port.to_string();
            let replica = common::Server::start_with(&["--replicaof", "127.0.0.1", &port]);
            let mut rc = Client::new(&replica);
            let until = Instant::now() + Duration::from_secs(3);
            while rc.call(&[b"VCARD", b"v"]) != integer(2) && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(10));
            }
            assert_eq!(rc.call(&[b"VCARD", b"v"]), integer(2));
            assert_eq!(rc.call(&[b"VCARD", b"precise"]), integer(1));
            assert_eq!(c.call(&[b"VREM", b"v", b"b"]), integer(1));
            let until = Instant::now() + Duration::from_secs(3);
            while rc.call(&[b"VCARD", b"v"]) != integer(1) && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(10));
            }
            assert_eq!(
                rc.call(&[b"VEMB", b"v", b"a\0"]),
                array(vec![bulk(b"1"), bulk(b"0")])
            );
            if mode == "--appendonly" {
                assert_eq!(
                    c.call(&[b"BGREWRITEAOF"]),
                    Reply::Simple(b"Background append only file rewriting started".to_vec())
                );
                let until = Instant::now() + Duration::from_secs(5);
                loop {
                    let Reply::Bulk(Some(info)) = c.call(&[b"INFO", b"persistence"]) else {
                        panic!("missing INFO")
                    };
                    if info
                        .windows(b"aof_rewrites:1\r\n".len())
                        .any(|s| s == b"aof_rewrites:1\r\n")
                    {
                        break;
                    }
                    assert!(Instant::now() < until, "AOF rewrite did not finish");
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        }
        {
            let server = common::Server::start_with(&[mode, filename]);
            let mut c = Client::new(&server);
            assert_eq!(c.call(&[b"VCARD", b"v"]), integer(1));
            assert_eq!(c.call(&[b"VDIM", b"v"]), integer(2));
            assert_eq!(c.call(&[b"VDIM", b"precise"]), integer(2));
            assert_eq!(
                c.call(&[b"VADD", b"precise", b"VALUES", b"2", b"0", b"1", b"b"]),
                error(b"ERR asked quantization mismatch with existing vector set")
            );
            assert_eq!(
                c.call(&[b"VADD", b"precise", b"VALUES", b"2", b"0", b"1", b"b", b"NOQUANT"]),
                integer(1)
            );
            assert_eq!(
                c.call(&[b"VEMB", b"v", b"a\0"]),
                array(vec![bulk(b"1"), bulk(b"0")])
            );
            let Reply::Integer(ttl) = c.call(&[b"PTTL", b"v"]) else {
                panic!("expected TTL")
            };
            assert!((0..=30000).contains(&ttl));
        }
        std::fs::remove_file(path).unwrap();
    }
}

#[test]
fn redis_rdb_export_rejects_vectors_explicitly() {
    let dir = common::temp_path("reru-vector-rdb", "");
    std::fs::create_dir(&dir).unwrap();
    let server =
        common::Server::start_with(&["--dir", dir.to_str().unwrap(), "--dbfilename", "vector.rdb"]);
    let mut c = Client::new(&server);
    assert_eq!(
        c.call(&[b"VADD", b"v", b"VALUES", b"2", b"1", b"0", b"a"]),
        integer(1)
    );
    assert!(
        matches!(c.call(&[b"SAVE"]), Reply::Error(e) if e.windows(b"vector sets".len()).any(|s| s == b"vector sets"))
    );
    assert!(
        matches!(c.call(&[b"BGSAVE"]), Reply::Error(e) if e.windows(b"vector sets".len()).any(|s| s == b"vector sets"))
    );
    assert!(
        matches!(c.call(&[b"DUMP", b"v"]), Reply::Error(e) if e.windows(b"vector sets".len()).any(|s| s == b"vector sets"))
    );
    assert!(!dir.join("vector.rdb").exists());
    drop(server);
    std::fs::remove_dir_all(dir).unwrap();
}
