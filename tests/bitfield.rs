// SPDX-License-Identifier: AGPL-3.0-only
#[path = "common/client.rs"]
#[allow(dead_code)]
mod client;
#[allow(dead_code)]
mod common;

use client::{array, integer, ok, Client, Reply};

#[test]
fn bitfield_chains_signed_fields_and_overflow_policies() {
    let server = common::Server::start();
    let mut client = Client::new(&server);
    assert_eq!(
        client.call(&[
            b"BITFIELD",
            b"bits",
            b"SET",
            b"i5",
            b"#0",
            b"31",
            b"GET",
            b"i5",
            b"#0",
            b"INCRBY",
            b"i5",
            b"#0",
            b"1",
        ]),
        array(vec![integer(0), integer(-1), integer(0)])
    );
    assert_eq!(
        client.call(&[
            b"BITFIELD",
            b"bits",
            b"OVERFLOW",
            b"SAT",
            b"SET",
            b"u2",
            b"#10",
            b"9",
            b"OVERFLOW",
            b"FAIL",
            b"INCRBY",
            b"u2",
            b"#10",
            b"1",
            b"GET",
            b"u2",
            b"#10",
        ]),
        array(vec![integer(0), Reply::Bulk(None), integer(3)])
    );
    assert_eq!(
        client.call(&[
            b"BITFIELD_RO",
            b"bits",
            b"GET",
            b"i5",
            b"#0",
            b"GET",
            b"u2",
            b"#10"
        ]),
        array(vec![integer(0), integer(3)])
    );
    assert!(matches!(
        client.call(&[b"BITFIELD_RO", b"bits", b"SET", b"u2", b"0", b"1"]),
        Reply::Error(_)
    ));
}

#[test]
fn bitfield_failed_writes_and_invalid_offsets_preserve_the_key() {
    let server = common::Server::start();
    let mut client = Client::new(&server);
    assert_eq!(client.call(&[b"SET", b"bits", b"A"]), ok());
    assert_eq!(client.call(&[b"PEXPIRE", b"bits", b"10000"]), integer(1));
    assert!(matches!(
        client.call(&[b"BITFIELD", b"bits", b"SET", b"u8", b"536870905", b"1"]),
        Reply::Error(_)
    ));
    assert_eq!(
        client.call(&[
            b"BITFIELD",
            b"bits",
            b"OVERFLOW",
            b"FAIL",
            b"SET",
            b"u2",
            b"0",
            b"9"
        ]),
        array(vec![Reply::Bulk(None)])
    );
    assert_eq!(
        client.call(&[b"GET", b"bits"]),
        Reply::Bulk(Some(b"A".to_vec()))
    );
    let Reply::Integer(ttl) = client.call(&[b"PTTL", b"bits"]) else {
        panic!("expected TTL")
    };
    assert!((9000..=10000).contains(&ttl));
}

#[test]
fn bitfield_only_invalidates_watch_after_a_successful_write() {
    let server = common::Server::start();
    let mut writer = Client::new(&server);
    let mut watcher = Client::new(&server);
    assert_eq!(writer.call(&[b"SET", b"bits", b"A"]), ok());
    assert_eq!(watcher.call(&[b"WATCH", b"bits"]), ok());
    assert_eq!(
        writer.call(&[b"BITFIELD_RO", b"bits", b"GET", b"u8", b"0"]),
        array(vec![integer(65)])
    );
    assert_eq!(
        writer.call(&[
            b"BITFIELD",
            b"bits",
            b"OVERFLOW",
            b"FAIL",
            b"SET",
            b"u2",
            b"0",
            b"9"
        ]),
        array(vec![Reply::Bulk(None)])
    );
    assert_eq!(watcher.call(&[b"MULTI"]), ok());
    assert_eq!(watcher.call(&[b"EXEC"]), array(vec![]));
    assert_eq!(watcher.call(&[b"WATCH", b"bits"]), ok());
    assert_eq!(
        writer.call(&[b"BITFIELD", b"bits", b"SET", b"u8", b"0", b"65"]),
        array(vec![integer(65)])
    );
    assert_eq!(watcher.call(&[b"MULTI"]), ok());
    assert_eq!(watcher.call(&[b"EXEC"]), Reply::Array(None));
}
