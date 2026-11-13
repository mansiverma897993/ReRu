// SPDX-License-Identifier: AGPL-3.0-only
#[path = "common/client.rs"]
#[allow(dead_code)]
mod client;
#[allow(dead_code)]
mod common;

use client::{array, integer, ok, Client, Reply};
use common::Server;
use std::time::{Duration, Instant};

fn path() -> std::path::PathBuf {
    common::temp_path("reru-aof-rewrite", ".aof")
}
fn bulk(bytes: &[u8]) -> Reply {
    Reply::Bulk(Some(bytes.to_vec()))
}
fn info(c: &mut Client) -> String {
    let Reply::Bulk(Some(bytes)) = c.call(&[b"INFO", b"persistence"]) else {
        panic!("expected INFO bulk string")
    };
    String::from_utf8(bytes).unwrap()
}
fn start(c: &mut Client) {
    assert_eq!(
        c.call(&[b"BGREWRITEAOF"]),
        Reply::Simple(b"Background append only file rewriting started".to_vec())
    );
}
fn wait(c: &mut Client, completed: u64) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let report = info(c);
        if report.contains("aof_rewrite_in_progress:0\r\n") {
            assert!(
                report.contains("aof_last_bgrewrite_status:ok\r\n"),
                "{report}"
            );
            assert!(
                report.contains(&format!("aof_rewrites:{completed}\r\n")),
                "{report}"
            );
            return;
        }
        assert!(Instant::now() < deadline, "rewrite did not finish");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn rewrite_compacts_history_and_restores_all_types_expiry_and_stream_last_id() {
    let path = path();
    let filename = path.to_str().unwrap();
    {
        let server = Server::start_with(&["--appendonly", filename]);
        let mut c = Client::new(&server);
        for _ in 0..40 {
            assert_eq!(c.call(&[b"SET", b"history", &[b'x'; 4096]]), ok());
        }
        c.call(&[b"DEL", b"history"]);
        c.call(&[b"SET", b"s\0", b"v\xff\r\n", b"PX", b"60000"]);
        c.call(&[b"RPUSH", b"list", b"a", b"b"]);
        c.call(&[b"HSET", b"hash", b"f", b"v"]);
        c.call(&[b"SADD", b"set", b"one", b"two"]);
        c.call(&[b"ZADD", b"zset", b"1.5", b"member"]);
        c.call(&[b"XADD", b"stream", b"5-0", b"f", b"v"]);
        c.call(&[b"XDEL", b"stream", b"5-0"]);
        let old_size = std::fs::metadata(&path).unwrap().len();
        start(&mut c);
        wait(&mut c, 1);
        assert!(std::fs::metadata(&path).unwrap().len() < old_size / 10);
        assert_eq!(c.call(&[b"GET", b"s\0"]), bulk(b"v\xff\r\n"));
    }
    {
        let server = Server::start_with(&["--appendonly", filename]);
        let mut c = Client::new(&server);
        assert_eq!(c.call(&[b"GET", b"history"]), Reply::Bulk(None));
        assert_eq!(c.call(&[b"GET", b"s\0"]), bulk(b"v\xff\r\n"));
        assert!(matches!(c.call(&[b"PTTL", b"s\0"]), Reply::Integer(n) if n > 0 && n <= 60000));
        assert_eq!(
            c.call(&[b"LRANGE", b"list", b"0", b"-1"]),
            array(vec![bulk(b"a"), bulk(b"b")])
        );
        assert_eq!(c.call(&[b"HGET", b"hash", b"f"]), bulk(b"v"));
        assert_eq!(c.call(&[b"SCARD", b"set"]), integer(2));
        assert_eq!(c.call(&[b"ZSCORE", b"zset", b"member"]), bulk(b"1.5"));
        assert_eq!(c.call(&[b"XLEN", b"stream"]), integer(0));
        assert!(matches!(
            c.call(&[b"XADD", b"stream", b"4-0", b"f", b"v"]),
            Reply::Error(_)
        ));
        assert_eq!(
            c.call(&[b"XADD", b"stream", b"6-0", b"f", b"v"]),
            bulk(b"6-0")
        );
    }
    std::fs::remove_file(path).unwrap();
}

#[test]
fn writes_transactions_flush_and_expiry_survive_rewrite_and_restart() {
    let path = path();
    let filename = path.to_str().unwrap();
    {
        let server = Server::start_with(&["--appendonly", filename]);
        let mut c = Client::new(&server);
        for i in 0..20 {
            c.call(&[b"SET", format!("large-{i}").as_bytes(), &[b'x'; 65536]]);
        }
        start(&mut c);
        assert_eq!(c.call(&[b"FLUSHDB"]), ok());
        for _ in 0..20 {
            c.call(&[b"MULTI"]);
            c.call(&[b"INCR", b"count"]);
            c.call(&[b"RPUSH", b"list", b"v"]);
            assert!(matches!(c.call(&[b"EXEC"]), Reply::Array(Some(_))));
        }
        c.call(&[b"SET", b"expire", b"v", b"PX", b"100"]);
        wait(&mut c, 1);
        std::thread::sleep(Duration::from_millis(250));
        start(&mut c);
        wait(&mut c, 2);
    }
    {
        let server = Server::start_with(&["--appendonly", filename]);
        let mut c = Client::new(&server);
        assert_eq!(c.call(&[b"GET", b"count"]), bulk(b"20"));
        assert_eq!(c.call(&[b"LLEN", b"list"]), integer(20));
        assert_eq!(c.call(&[b"DBSIZE"]), integer(2));
    }
    std::fs::remove_file(path).unwrap();
}

#[test]
fn list_chunks_survive_aof_rewrite_and_restart() {
    // The rewrite emits RPUSH in chunks of 1022 elements, so a list longer than
    // one chunk must survive the boundary without losing or repeating members.
    let path = path();
    let filename = path.to_str().unwrap();
    let members: Vec<Vec<u8>> = (0..2045).map(|i| format!("v{i}").into_bytes()).collect();
    {
        let server = Server::start_with(&["--appendonly", filename]);
        let mut c = Client::new(&server);
        let mut pushed = 0i64;
        for batch in members.chunks(900) {
            let mut command: Vec<&[u8]> = vec![b"RPUSH", b"list"];
            command.extend(batch.iter().map(Vec::as_slice));
            pushed += batch.len() as i64;
            assert_eq!(c.call(&command), integer(pushed));
        }
        start(&mut c);
        wait(&mut c, 1);
    }
    {
        let server = Server::start_with(&["--appendonly", filename]);
        let mut c = Client::new(&server);
        assert_eq!(c.call(&[b"LLEN", b"list"]), integer(2045));
        assert_eq!(
            c.call(&[b"LRANGE", b"list", b"0", b"2"]),
            array(vec![bulk(b"v0"), bulk(b"v1"), bulk(b"v2")])
        );
        assert_eq!(
            c.call(&[b"LRANGE", b"list", b"-3", b"-1"]),
            array(vec![bulk(b"v2042"), bulk(b"v2043"), bulk(b"v2044")])
        );
    }
    std::fs::remove_file(path).unwrap();
}

#[test]
fn rewrite_errors_preserve_data_and_allow_retry() {
    let disabled = Server::start();
    let mut c = Client::new(&disabled);
    assert!(info(&mut c).contains("aof_enabled:0\r\n"));
    assert_eq!(
        c.call(&[b"CONFIG", b"GET", b"appendonly", b"appendfsync"]),
        array(vec![
            bulk(b"appendonly"),
            bulk(b"no"),
            bulk(b"appendfsync"),
            bulk(b"always")
        ])
    );
    assert!(matches!(c.call(&[b"BGREWRITEAOF"]), Reply::Error(_)));
    let path = path();
    let filename = path.to_str().unwrap();
    {
        let server = Server::start_with(&["--appendonly", filename]);
        let mut c = Client::new(&server);
        c.call(&[b"SET", b"keep", b"value"]);
        assert_eq!(
            c.call(&[b"CONFIG", b"GET", b"appendonly"]),
            array(vec![bulk(b"appendonly"), bulk(b"yes")])
        );
        assert!(matches!(
            c.call(&[b"BGREWRITEAOF", b"extra"]),
            Reply::Error(_)
        ));
        c.call(&[b"MULTI"]);
        assert!(matches!(c.call(&[b"BGREWRITEAOF"]), Reply::Error(_)));
        assert!(matches!(c.call(&[b"EXEC"]), Reply::Error(e) if e.starts_with(b"EXECABORT")));
        let backup = path.with_extension("backup");
        std::fs::rename(&path, &backup).unwrap();
        assert!(matches!(c.call(&[b"BGREWRITEAOF"]), Reply::Error(_)));
        assert!(info(&mut c).contains("aof_last_bgrewrite_status:err\r\n"));
        std::fs::rename(&backup, &path).unwrap();
        start(&mut c);
        wait(&mut c, 1);
        assert_eq!(c.call(&[b"GET", b"keep"]), bulk(b"value"));
    }
    std::fs::remove_file(path).unwrap();
}
