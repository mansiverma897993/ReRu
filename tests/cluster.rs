// SPDX-License-Identifier: AGPL-3.0-only
#[path = "common/client.rs"]
mod client;
mod common;

use client::{array, integer, ok, queued, Client, Reply};
use std::fs;
use std::net::TcpListener;
use std::process::Command;

const FIRST_ID: &str = "1111111111111111111111111111111111111111";
const SECOND_ID: &str = "2222222222222222222222222222222222222222";

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

fn pair() -> (common::Server, common::Server) {
    let first_port = free_port();
    let mut second_port = free_port();
    while first_port == second_port {
        second_port = free_port();
    }
    let first = format!("{FIRST_ID}@127.0.0.1:{first_port}@0-8191");
    let second = format!("{SECOND_ID}@127.0.0.1:{second_port}@8192-16383");
    let a = common::Server::start_with(&[
        "--port",
        &first_port.to_string(),
        "--cluster-node",
        &first,
        "--cluster-node",
        &second,
    ]);
    let b = common::Server::start_with(&[
        "--port",
        &second_port.to_string(),
        "--cluster-node",
        &first,
        "--cluster-node",
        &second,
    ]);
    (a, b)
}

#[test]
fn standalone_mode_is_unchanged() {
    let server = common::Server::start();
    let mut client = Client::new(&server);
    assert_eq!(client.call(&[b"SET", b"foo", b"standalone"]), ok());
    assert_eq!(
        client.call(&[b"GET", b"foo"]),
        Reply::Bulk(Some(b"standalone".to_vec()))
    );
    assert_eq!(
        client.call(&[b"CLUSTER", b"KEYSLOT", b"foo"]),
        Reply::Error(b"ERR This instance has cluster support disabled".to_vec())
    );
}

#[test]
fn slots_and_binary_hash_tags_match_redis_vectors() {
    let (a, _) = pair();
    let mut client = Client::new(&a);
    assert_eq!(
        client.call(&[b"CLUSTER", b"KEYSLOT", b"foo"]),
        integer(12182)
    );
    assert_eq!(
        client.call(&[b"CLUSTER", b"KEYSLOT", b"bar"]),
        integer(5061)
    );
    assert_eq!(
        client.call(&[b"CLUSTER", b"KEYSLOT", b"{user1000}.following"]),
        integer(3443)
    );
    assert_eq!(
        client.call(&[b"CLUSTER", b"KEYSLOT", b"{user1000}.followers"]),
        integer(3443)
    );
    assert_eq!(
        client.call(&[b"CLUSTER", b"KEYSLOT", b"a\0b"]),
        integer(8383)
    );
    assert_eq!(
        client.call(&[b"CLUSTER", b"KEYSLOT"]),
        Reply::Error(b"ERR wrong number of arguments for 'cluster|keyslot' command".to_vec())
    );
    assert_eq!(
        client.call(&[b"CLUSTER", b"BOGUS"]),
        Reply::Error(b"ERR unknown subcommand 'BOGUS'. Try CLUSTER HELP.".to_vec())
    );
}

#[test]
fn static_nodes_route_keys_and_reject_cross_slot_writes() {
    let (a, b) = pair();
    let mut left = Client::new(&a);
    let mut right = Client::new(&b);
    let left_endpoint = format!("127.0.0.1:{}", a.port);
    let right_endpoint = format!("127.0.0.1:{}", b.port);

    assert_eq!(left.call(&[b"SET", b"bar", b"left"]), ok());
    assert_eq!(left.call(&[b"SET", b"{bar}:other", b"same-slot"]), ok());
    assert_eq!(right.call(&[b"SET", b"foo", b"right"]), ok());
    assert_eq!(
        left.call(&[b"GET", b"foo"]),
        Reply::Error(format!("MOVED 12182 {right_endpoint}").into_bytes())
    );
    assert_eq!(
        right.call(&[b"GET", b"bar"]),
        Reply::Error(format!("MOVED 5061 {left_endpoint}").into_bytes())
    );
    assert_eq!(
        left.call(&[b"MGET", b"bar", b"foo"]),
        Reply::Error(b"CROSSSLOT Keys in request don't hash to the same slot".to_vec())
    );
    assert_eq!(
        left.call(&[b"MSET", b"bar", b"changed", b"foo", b"bad"]),
        Reply::Error(b"CROSSSLOT Keys in request don't hash to the same slot".to_vec())
    );
    assert_eq!(
        left.call(&[b"WATCH", b"foo"]),
        Reply::Error(format!("MOVED 12182 {right_endpoint}").into_bytes())
    );
    assert_eq!(left.call(&[b"SELECT", b"0"]), ok());
    assert_eq!(
        left.call(&[b"SELECT", b"1"]),
        Reply::Error(b"ERR SELECT is not allowed in cluster mode".to_vec())
    );
    assert_eq!(
        left.call(&[b"GET", b"bar"]),
        Reply::Bulk(Some(b"left".to_vec()))
    );
    assert_eq!(
        right.call(&[b"GET", b"foo"]),
        Reply::Bulk(Some(b"right".to_vec()))
    );
}

#[test]
fn transaction_cross_slot_is_rejected_before_any_write() {
    let (a, b) = pair();
    let mut client = Client::new(&a);
    assert_eq!(client.call(&[b"MULTI"]), ok());
    assert_eq!(client.call(&[b"SET", b"bar", b"one"]), queued());
    assert_eq!(client.call(&[b"SET", b"abc", b"two"]), queued());
    assert_eq!(
        client.call(&[b"EXEC"]),
        Reply::Error(b"CROSSSLOT Keys in request don't hash to the same slot".to_vec())
    );
    assert_eq!(client.call(&[b"GET", b"bar"]), Reply::Bulk(None));
    assert_eq!(client.call(&[b"MULTI"]), ok());
    assert_eq!(
        client.call(&[b"SET", b"foo", b"remote"]),
        Reply::Error(format!("MOVED 12182 127.0.0.1:{}", b.port).into_bytes())
    );
    assert_eq!(
        client.call(&[b"EXEC"]),
        Reply::Error(b"EXECABORT Transaction discarded because of previous errors.".to_vec())
    );
}

#[test]
fn topology_replies_report_both_slot_owners() {
    let (a, b) = pair();
    let mut client = Client::new(&a);
    let node = |server: &common::Server, id: &str| {
        array(vec![
            Reply::Bulk(Some(b"127.0.0.1".to_vec())),
            integer(i64::from(server.port)),
            Reply::Bulk(Some(id.as_bytes().to_vec())),
            array(vec![]),
        ])
    };
    assert_eq!(
        client.call(&[b"CLUSTER", b"SLOTS"]),
        array(vec![
            array(vec![integer(0), integer(8191), node(&a, FIRST_ID)]),
            array(vec![integer(8192), integer(16383), node(&b, SECOND_ID)]),
        ])
    );
    let shard = |start: i64, end: i64, server: &common::Server, id: &str| {
        array(vec![
            Reply::Bulk(Some(b"slots".to_vec())),
            array(vec![integer(start), integer(end)]),
            Reply::Bulk(Some(b"nodes".to_vec())),
            array(vec![array(vec![
                Reply::Bulk(Some(b"id".to_vec())),
                Reply::Bulk(Some(id.as_bytes().to_vec())),
                Reply::Bulk(Some(b"port".to_vec())),
                integer(i64::from(server.port)),
                Reply::Bulk(Some(b"ip".to_vec())),
                Reply::Bulk(Some(b"127.0.0.1".to_vec())),
                Reply::Bulk(Some(b"endpoint".to_vec())),
                Reply::Bulk(Some(b"127.0.0.1".to_vec())),
                Reply::Bulk(Some(b"role".to_vec())),
                Reply::Bulk(Some(b"master".to_vec())),
                Reply::Bulk(Some(b"replication-offset".to_vec())),
                integer(0),
                Reply::Bulk(Some(b"health".to_vec())),
                Reply::Bulk(Some(b"online".to_vec())),
            ])]),
        ])
    };
    assert_eq!(
        client.call(&[b"CLUSTER", b"SHARDS"]),
        array(vec![
            shard(0, 8191, &a, FIRST_ID),
            shard(8192, 16383, &b, SECOND_ID)
        ])
    );
    let Reply::Array(Some(hello)) = client.call(&[b"HELLO", b"2"]) else {
        panic!("expected HELLO array");
    };
    assert!(hello.windows(2).any(|pair| {
        pair == [
            Reply::Bulk(Some(b"mode".to_vec())),
            Reply::Bulk(Some(b"cluster".to_vec())),
        ]
    }));
}

#[test]
fn invalid_or_incomplete_static_maps_fail_at_startup() {
    let examples = [
        (
            format!("{FIRST_ID}@127.0.0.1:7001@0-8191"),
            format!("{SECOND_ID}@127.0.0.1:7002@8191-16383"),
        ),
        (
            format!("{FIRST_ID}@127.0.0.1:7001@0-8190"),
            format!("{SECOND_ID}@127.0.0.1:7002@8192-16383"),
        ),
        (
            format!("{FIRST_ID}@127.0.0.1:7001@0-8191"),
            format!("{FIRST_ID}@127.0.0.1:7002@8192-16383"),
        ),
    ];
    for (first, second) in examples {
        let output = Command::new(env!("CARGO_BIN_EXE_reru"))
            .args([
                "--port",
                "0",
                "--cluster-node",
                &first,
                "--cluster-node",
                &second,
            ])
            .output()
            .unwrap();
        assert!(!output.status.success(), "invalid map was accepted");
        assert!(
            String::from_utf8_lossy(&output.stderr)
                .contains("cluster slots must be complete and disjoint with unique nodes"),
            "unexpected error: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn persisted_keys_outside_the_local_slots_are_rejected() {
    let path = common::temp_path("reru-cluster", ".snap");
    let path_str = path.to_str().unwrap();
    {
        let server = common::Server::start_with(&["--snapshot", path_str]);
        let mut client = Client::new(&server);
        assert_eq!(client.call(&[b"SET", b"foo", b"remote"]), ok());
    }
    let port = free_port();
    let first = format!("{FIRST_ID}@127.0.0.1:{port}@0-8191");
    let second = format!("{SECOND_ID}@127.0.0.1:1@8192-16383");
    let output = Command::new(env!("CARGO_BIN_EXE_reru"))
        .args([
            "--port",
            &port.to_string(),
            "--snapshot",
            path_str,
            "--cluster-node",
            &first,
            "--cluster-node",
            &second,
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr)
        .contains("persisted dataset contains a key outside local cluster slots"));
    fs::remove_file(path).unwrap();
}
