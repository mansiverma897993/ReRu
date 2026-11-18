// SPDX-License-Identifier: AGPL-3.0-only
#[path = "common/client.rs"]
#[allow(dead_code)]
mod client;
#[allow(dead_code)]
mod common;

use client::{array, integer, ok, Client, Reply};
use common::Server;
use std::time::{SystemTime, UNIX_EPOCH};

fn bulk(value: &[u8]) -> Reply {
    Reply::Bulk(Some(value.to_vec()))
}

#[test]
fn keys_and_scan_filter_live_keys_by_glob_and_type() {
    let server = Server::start();
    let mut client = Client::new(&server);
    assert_eq!(client.call(&[b"SET", b"user:a", b"1"]), ok());
    assert_eq!(client.call(&[b"SET", b"user:b", b"2"]), ok());
    assert_eq!(
        client.call(&[b"HSET", b"user:h", b"field", b"value"]),
        integer(1)
    );
    assert_eq!(client.call(&[b"SET", b"other", b"x", b"PX", b"1"]), ok());
    std::thread::sleep(std::time::Duration::from_millis(5));
    assert_eq!(
        client.call(&[b"KEYS", b"user:*"]),
        array(vec![bulk(b"user:a"), bulk(b"user:b"), bulk(b"user:h")])
    );
    assert_eq!(client.call(&[b"KEYS", b"USER:*"]), array(vec![]));
    assert_eq!(
        client.call(&[b"KEYS", b"*"]),
        array(vec![bulk(b"user:a"), bulk(b"user:b"), bulk(b"user:h")])
    );
    let mut cursor = b"0".to_vec();
    let mut found = Vec::new();
    for _ in 0..10 {
        let response = client.call(&[
            b"SCAN", &cursor, b"COUNT", b"1", b"MATCH", b"user:*", b"TYPE", b"string",
        ]);
        let Reply::Array(Some(mut outer)) = response else {
            panic!("SCAN must return two elements");
        };
        let Reply::Bulk(Some(next)) = outer.remove(0) else {
            panic!("SCAN cursor must be a bulk string");
        };
        let Reply::Array(Some(page)) = outer.remove(0) else {
            panic!("SCAN page must be an array");
        };
        found.extend(page);
        cursor = next;
        if cursor == b"0" {
            break;
        }
    }
    assert_eq!(cursor, b"0");
    assert_eq!(found, vec![bulk(b"user:a"), bulk(b"user:b")]);
    assert!(matches!(
        client.call(&[b"RANDOMKEY"]),
        Reply::Bulk(Some(key)) if [b"user:a".as_slice(), b"user:b", b"user:h"].contains(&key.as_slice())
    ));
}

#[test]
fn randomkey_and_expiretime_handle_missing_and_absolute_deadlines() {
    let server = Server::start();
    let mut client = Client::new(&server);
    assert_eq!(client.call(&[b"RANDOMKEY"]), Reply::Bulk(None));
    assert_eq!(client.call(&[b"EXPIRETIME", b"missing"]), integer(-2));
    assert_eq!(client.call(&[b"SET", b"only", b"value"]), ok());
    assert_eq!(client.call(&[b"RANDOMKEY"]), bulk(b"only"));
    assert_eq!(client.call(&[b"PEXPIRETIME", b"only"]), integer(-1));
    let future = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
        + 60000;
    assert_eq!(
        client.call(&[b"PEXPIREAT", b"only", future.to_string().as_bytes()]),
        integer(1)
    );
    assert!(
        matches!(client.call(&[b"PEXPIRETIME", b"only"]), Reply::Integer(value) if (future - value).abs() <= 2)
    );
    assert!(
        matches!(client.call(&[b"EXPIRETIME", b"only"]), Reply::Integer(value) if (future / 1000 - value).abs() <= 1)
    );
}

#[test]
fn expiretime_rounds_the_absolute_second_half_up() {
    let server = Server::start();
    let mut client = Client::new(&server);
    let base = (SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
        / 1000
        + 200)
        * 1000;
    // 499/500/501 are ambiguous under the 1 ms monotonic drift; 300 and 700
    // exercise both sides of the half-second rule.
    for offset in [0i64, 1, 300, 700, 999, 1000] {
        let deadline = base + offset;
        assert_eq!(client.call(&[b"SET", b"k", b"v"]), ok());
        assert_eq!(
            client.call(&[b"PEXPIREAT", b"k", deadline.to_string().as_bytes()]),
            integer(1)
        );
        // The key stores a monotonic deadline, so allow a millisecond of drift.
        let Reply::Integer(pexpire) = client.call(&[b"PEXPIRETIME", b"k"]) else {
            panic!("PEXPIRETIME must return an integer")
        };
        assert!((pexpire - deadline).abs() <= 1, "{offset}");
        assert_eq!(
            client.call(&[b"EXPIRETIME", b"k"]),
            integer(deadline / 1000 + i64::from(offset % 1000 >= 500)),
            "{offset}"
        );
    }
}

#[test]
fn absolute_set_and_getex_expiry_round_expiretime() {
    let server = Server::start();
    let mut client = Client::new(&server);
    let base = (SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
        / 1000
        + 200)
        * 1000;
    for offset in [300i64, 700, 999] {
        let deadline = base + offset;
        let deadline = deadline.to_string();
        assert_eq!(
            client.call(&[b"SET", b"k", b"v", b"PXAT", deadline.as_bytes()]),
            ok()
        );
        let Reply::Integer(pexpire) = client.call(&[b"PEXPIRETIME", b"k"]) else {
            panic!("PEXPIRETIME must return an integer")
        };
        assert!((pexpire - deadline.parse::<i64>().unwrap()).abs() <= 1);
        assert_eq!(
            client.call(&[b"EXPIRETIME", b"k"]),
            integer(base / 1000 + i64::from(offset >= 500)),
            "{offset}"
        );

        assert_eq!(
            client.call(&[b"GETEX", b"k", b"PXAT", deadline.as_bytes()]),
            bulk(b"v")
        );
        let Reply::Integer(pexpire) = client.call(&[b"PEXPIRETIME", b"k"]) else {
            panic!("PEXPIRETIME must return an integer")
        };
        assert!((pexpire - deadline.parse::<i64>().unwrap()).abs() <= 1);
        assert_eq!(
            client.call(&[b"EXPIRETIME", b"k"]),
            integer(base / 1000 + i64::from(offset >= 500)),
            "{offset}"
        );
    }
}
