// SPDX-License-Identifier: AGPL-3.0-only
#[path = "common/client.rs"]
#[allow(dead_code)]
mod client;
#[allow(dead_code)]
mod common;

use client::{array, integer, ok, Client, Reply};
use common::Server;
use std::time::{SystemTime, UNIX_EPOCH};

fn path() -> std::path::PathBuf {
    common::temp_path("reru-aof", ".aof")
}

fn bulk(value: &[u8]) -> Reply {
    Reply::Bulk(Some(value.to_vec()))
}

#[test]
fn aof_restores_types_binary_values_and_absolute_expiry() {
    let path = path();
    let filename = path.to_str().unwrap();
    {
        let server = Server::start_with(&["--appendonly", filename]);
        let mut client = Client::new(&server);
        assert_eq!(client.call(&[b"SET", b"s", b"a\0b", b"PX", b"60000"]), ok());
        assert_eq!(client.call(&[b"RPUSH", b"l", b"a", b"b"]), integer(2));
        assert_eq!(client.call(&[b"HSET", b"h", b"f", b"v"]), integer(1));
        assert_eq!(client.call(&[b"SADD", b"members", b"a", b"b"]), integer(2));
        assert_eq!(client.call(&[b"ZADD", b"z", b"2", b"item"]), integer(1));
        assert_eq!(
            client.call(&[b"XADD", b"stream", b"1-0", b"field", b"value"]),
            bulk(b"1-0")
        );
        assert_eq!(client.call(&[b"SETBIT", b"bitmap", b"1", b"1"]), integer(0));
        assert_eq!(client.call(&[b"SET", b"gone", b"x"]), ok());
        assert_eq!(client.call(&[b"DEL", b"gone"]), integer(1));
    }
    let bytes = std::fs::read(&path).unwrap();
    assert!(bytes.starts_with(b"*1\r\n$5\r\nMULTI\r\n"));
    {
        let server = Server::start_with(&["--appendonly", filename]);
        let mut client = Client::new(&server);
        assert_eq!(client.call(&[b"GET", b"s"]), bulk(b"a\0b"));
        assert!(
            matches!(client.call(&[b"PTTL", b"s"]), Reply::Integer(value) if value > 0 && value <= 60000)
        );
        assert_eq!(
            client.call(&[b"LRANGE", b"l", b"0", b"-1"]),
            array(vec![bulk(b"a"), bulk(b"b")])
        );
        assert_eq!(client.call(&[b"HGET", b"h", b"f"]), bulk(b"v"));
        assert_eq!(client.call(&[b"SCARD", b"members"]), integer(2));
        assert_eq!(client.call(&[b"ZSCORE", b"z", b"item"]), bulk(b"2"));
        assert_eq!(client.call(&[b"XLEN", b"stream"]), integer(1));
        assert_eq!(client.call(&[b"GETBIT", b"bitmap", b"1"]), integer(1));
        assert_eq!(client.call(&[b"GET", b"gone"]), Reply::Bulk(None));
    }
    std::fs::remove_file(path).unwrap();
}

#[test]
fn aof_discards_incomplete_final_transaction() {
    let path = path();
    let filename = path.to_str().unwrap();
    {
        let server = Server::start_with(&["--appendonly", filename]);
        let mut client = Client::new(&server);
        assert_eq!(client.call(&[b"SET", b"good", b"kept"]), ok());
    }
    let complete_len = std::fs::metadata(&path).unwrap().len();
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    file.write_all(b"*1\r\n$5\r\nMULTI\r\n*3\r\n$3\r\nSET\r\n$3\r\nbad\r\n$4\r\nnope\r\n")
        .unwrap();
    drop(file);
    {
        let server = Server::start_with(&["--appendonly", filename]);
        let mut client = Client::new(&server);
        assert_eq!(client.call(&[b"GET", b"good"]), bulk(b"kept"));
        assert_eq!(client.call(&[b"GET", b"bad"]), Reply::Bulk(None));
        assert_eq!(std::fs::metadata(&path).unwrap().len(), complete_len);
        assert_eq!(client.call(&[b"SET", b"after", b"works"]), ok());
    }
    {
        let server = Server::start_with(&["--appendonly", filename]);
        let mut client = Client::new(&server);
        assert_eq!(client.call(&[b"GET", b"after"]), bulk(b"works"));
        assert_eq!(client.call(&[b"GET", b"bad"]), Reply::Bulk(None));
    }
    std::fs::remove_file(path).unwrap();
}

#[test]
fn pexpireat_uses_unix_milliseconds() {
    let server = Server::start();
    let mut client = Client::new(&server);
    let future = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis()
        + 60000;
    let future = future.to_string();
    assert_eq!(client.call(&[b"SET", b"key", b"value"]), ok());
    assert_eq!(
        client.call(&[b"PEXPIREAT", b"key", future.as_bytes()]),
        integer(1)
    );
    assert!(
        matches!(client.call(&[b"PTTL", b"key"]), Reply::Integer(value) if value > 0 && value <= 60000)
    );
    assert_eq!(client.call(&[b"PEXPIREAT", b"key", b"1"]), integer(1));
    assert_eq!(client.call(&[b"GET", b"key"]), Reply::Bulk(None));
}

#[test]
fn aof_restores_transaction_results_and_eviction() {
    let path = path();
    let filename = path.to_str().unwrap();
    let value = vec![b'x'; 100];
    {
        let server = Server::start_with(&[
            "--appendonly",
            filename,
            "--maxmemory",
            "330",
            "--maxmemory-policy",
            "allkeys-lru",
        ]);
        let mut client = Client::new(&server);
        assert_eq!(client.call(&[b"MULTI"]), ok());
        assert_eq!(
            client.call(&[b"SET", b"a", &value]),
            Reply::Simple(b"QUEUED".to_vec())
        );
        assert_eq!(
            client.call(&[b"SET", b"b", &value]),
            Reply::Simple(b"QUEUED".to_vec())
        );
        assert_eq!(client.call(&[b"EXEC"]), array(vec![ok(), ok()]));
        assert_eq!(client.call(&[b"SET", b"c", &value]), ok());
        assert_eq!(client.call(&[b"GET", b"a"]), Reply::Bulk(None));
    }
    {
        let server = Server::start_with(&[
            "--appendonly",
            filename,
            "--maxmemory",
            "330",
            "--maxmemory-policy",
            "allkeys-lru",
        ]);
        let mut client = Client::new(&server);
        assert_eq!(client.call(&[b"GET", b"a"]), Reply::Bulk(None));
        assert_eq!(client.call(&[b"GET", b"b"]), bulk(&value));
        assert_eq!(client.call(&[b"GET", b"c"]), bulk(&value));
    }
    std::fs::remove_file(path).unwrap();
}
