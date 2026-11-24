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
fn live_slot_migration_preserves_binary_keys_ttl_and_restart_topology() {
    let first_port = free_port();
    let second_port = free_port();
    let first = format!("{FIRST_ID}@127.0.0.1:{first_port}@0-8191");
    let second = format!("{SECOND_ID}@127.0.0.1:{second_port}@8192-16383");
    let first_state = common::temp_path("reru-cluster-first", ".state");
    let second_state = common::temp_path("reru-cluster-second", ".state");
    let first_data = common::temp_path("reru-cluster-first", ".snap");
    let second_data = common::temp_path("reru-cluster-second", ".snap");
    let first_args = [
        "--port",
        &first_port.to_string(),
        "--cluster-node",
        &first,
        "--cluster-node",
        &second,
        "--cluster-state",
        first_state.to_str().unwrap(),
        "--snapshot",
        first_data.to_str().unwrap(),
    ];
    let second_args = [
        "--port",
        &second_port.to_string(),
        "--cluster-node",
        &first,
        "--cluster-node",
        &second,
        "--cluster-state",
        second_state.to_str().unwrap(),
        "--snapshot",
        second_data.to_str().unwrap(),
    ];
    let key = b"{bar}:binary\0";
    let target = format!("127.0.0.1:{second_port}");
    {
        let a = common::Server::start_with(&first_args);
        let b = common::Server::start_with(&second_args);
        let mut source = Client::new(&a);
        let mut dest = Client::new(&b);
        assert_eq!(source.call(&[b"SET", key, b"\0\xff\r\n"]), ok());
        assert_eq!(source.call(&[b"PEXPIRE", key, b"30000"]), integer(1));
        assert_eq!(
            source.call(&[b"RPUSH", b"{bar}:list", b"a", b"b"]),
            integer(2)
        );
        assert_eq!(
            source.call(&[b"HSET", b"{bar}:hash", b"f", b"v"]),
            integer(1)
        );
        assert_eq!(
            source.call(&[b"ZADD", b"{bar}:zset", b"1", b"m"]),
            integer(1)
        );
        assert_eq!(
            source.call(&[b"VADD", b"{bar}:vector", b"VALUES", b"2", b"1", b"0", b"m"]),
            integer(1)
        );
        assert!(matches!(
            source.call(&[b"XADD", b"{bar}:stream", b"*", b"f", b"v"]),
            Reply::Bulk(Some(_))
        ));
        assert_eq!(source.call(&[b"SET", b"{bar}:race", b"0"]), ok());
        assert_eq!(source.call(&[b"SET", b"{bar}:conflict", b"source"]), ok());
        assert_eq!(
            source.call(&[b"CLUSTER", b"SETSLOT", b"16384", b"STABLE"]),
            Reply::Error(b"ERR Invalid or out of range slot".to_vec())
        );
        assert_eq!(
            source.call(&[b"CLUSTER", b"SETSLOT", b"5061", b"BAD"]),
            Reply::Error(
                b"ERR Invalid CLUSTER SETSLOT action or number of arguments. Try CLUSTER HELP"
                    .to_vec()
            )
        );
        assert_eq!(
            source.call(&[b"CLUSTER", b"SETSLOT", b"5061", b"MIGRATING", b"not-a-node"]),
            Reply::Error(b"ERR I don't know about node not-a-node".to_vec())
        );
        assert_eq!(
            dest.call(&[
                b"CLUSTER",
                b"SETSLOT",
                b"5061",
                b"IMPORTING",
                FIRST_ID.as_bytes()
            ]),
            ok()
        );
        assert_eq!(
            source.call(&[
                b"CLUSTER",
                b"SETSLOT",
                b"5061",
                b"MIGRATING",
                SECOND_ID.as_bytes()
            ]),
            ok()
        );
        let writer = std::thread::spawn(move || {
            let mut client = Client::on_port(first_port);
            let mut last = b"0".to_vec();
            for number in 1..=100 {
                let value = number.to_string().into_bytes();
                match client.call(&[b"SET", b"{bar}:race", &value]) {
                    reply if reply == ok() => last = value,
                    Reply::Error(error) if error.starts_with(b"ASK 5061 ") => break,
                    reply => panic!("unexpected concurrent write reply: {reply:?}"),
                }
            }
            last
        });
        assert_eq!(
            source.call(&[
                b"MIGRATE",
                b"127.0.0.1",
                second_port.to_string().as_bytes(),
                b"{bar}:race",
                b"0",
                b"1000"
            ]),
            ok()
        );
        let last_write = writer.join().unwrap();
        assert_eq!(dest.call(&[b"ASKING"]), ok());
        assert_eq!(
            dest.call(&[b"GET", b"{bar}:race"]),
            Reply::Bulk(Some(last_write))
        );
        assert_eq!(dest.call(&[b"ASKING"]), ok());
        assert_eq!(dest.call(&[b"SET", b"{bar}:conflict", b"target"]), ok());
        assert_eq!(
            source.call(&[
                b"MIGRATE",
                b"127.0.0.1",
                second_port.to_string().as_bytes(),
                b"{bar}:conflict",
                b"0",
                b"1000"
            ]),
            Reply::Error(b"BUSYKEY Target key name already exists.".to_vec())
        );
        assert_eq!(
            source.call(&[b"GET", b"{bar}:conflict"]),
            Reply::Bulk(Some(b"source".to_vec()))
        );
        assert_eq!(dest.call(&[b"ASKING"]), ok());
        assert_eq!(dest.call(&[b"DEL", b"{bar}:conflict"]), integer(1));
        assert_eq!(
            source.call(&[
                b"MIGRATE",
                b"127.0.0.1",
                second_port.to_string().as_bytes(),
                b"{bar}:conflict",
                b"0",
                b"1000"
            ]),
            ok()
        );
        assert_eq!(
            source.call(&[b"GET", b"{bar}:absent"]),
            Reply::Error(format!("ASK 5061 {target}").into_bytes())
        );
        assert_eq!(
            dest.call(&[b"GET", key]),
            Reply::Error(format!("MOVED 5061 127.0.0.1:{first_port}").into_bytes())
        );
        assert_eq!(dest.call(&[b"ASKING"]), ok());
        assert_eq!(dest.call(&[b"MULTI"]), ok());
        assert_eq!(dest.call(&[b"GET", key]), queued());
        assert_eq!(dest.call(&[b"EXEC"]), array(vec![Reply::Bulk(None)]));
        assert_eq!(
            dest.call(&[b"GET", key]),
            Reply::Error(format!("MOVED 5061 127.0.0.1:{first_port}").into_bytes())
        );
        assert_eq!(dest.call(&[b"ASKING"]), ok());
        assert_eq!(dest.call(&[b"GET", key]), Reply::Bulk(None));
        assert_eq!(
            dest.call(&[b"GET", key]),
            Reply::Error(format!("MOVED 5061 127.0.0.1:{first_port}").into_bytes())
        );
        assert_eq!(
            source.call(&[b"MGET", key, b"{bar}:absent"]),
            Reply::Error(b"TRYAGAIN Multiple keys request during rehashing of slot".to_vec())
        );
        assert_eq!(
            source.call(&[
                b"MIGRATE",
                b"127.0.0.1",
                second_port.to_string().as_bytes(),
                b"{bar}:missing",
                b"0",
                b"1000"
            ]),
            Reply::Simple(b"NOKEY".to_vec())
        );
        assert_eq!(
            source.call(&[b"MIGRATE", b"127.0.0.1", b"bad", key, b"0", b"1000"]),
            Reply::Error(b"ERR invalid target port".to_vec())
        );
        assert_eq!(
            source.call(&[b"MIGRATE", b"127.0.0.1", b"1234", key, b"1", b"1000"]),
            Reply::Error(b"ERR MIGRATE only supports database 0".to_vec())
        );
        assert_eq!(source.call(&[b"MULTI"]), ok());
        assert_eq!(
            source.call(&[b"GET", b"{bar}:absent"]),
            Reply::Error(format!("ASK 5061 {target}").into_bytes())
        );
        assert_eq!(
            source.call(&[b"EXEC"]),
            Reply::Error(b"EXECABORT Transaction discarded because of previous errors.".to_vec())
        );
        drop(dest);
        drop(b);
        assert_eq!(
            source.call(&[
                b"MIGRATE",
                b"127.0.0.1",
                second_port.to_string().as_bytes(),
                key,
                b"0",
                b"100"
            ]),
            Reply::Error(b"IOERR error or timeout writing to target instance".to_vec())
        );
        assert_eq!(
            source.call(&[b"GET", key]),
            Reply::Bulk(Some(b"\0\xff\r\n".to_vec()))
        );
        let b = common::Server::start_with(&second_args);
        let mut dest = Client::new(&b);
        assert_eq!(
            dest.call(&[
                b"CONFIG",
                b"SET",
                b"maxmemory",
                b"1",
                b"maxmemory-policy",
                b"allkeys-random"
            ]),
            ok()
        );
        assert_eq!(
            source.call(&[
                b"MIGRATE",
                b"127.0.0.1",
                second_port.to_string().as_bytes(),
                key,
                b"0",
                b"1000"
            ]),
            Reply::Error(b"OOM command not allowed when used memory > 'maxmemory'.".to_vec())
        );
        assert_eq!(
            source.call(&[b"GET", key]),
            Reply::Bulk(Some(b"\0\xff\r\n".to_vec()))
        );
        assert_eq!(dest.call(&[b"CONFIG", b"SET", b"maxmemory", b"0"]), ok());
        drop(dest);
        assert_eq!(
            source.call(&[
                b"MIGRATE",
                b"127.0.0.1",
                second_port.to_string().as_bytes(),
                key,
                b"0",
                b"1000"
            ]),
            ok()
        );
        assert_eq!(
            source.call(&[b"GET", key]),
            Reply::Error(format!("ASK 5061 {target}").into_bytes())
        );
        drop(b);
        let b = common::Server::start_with(&second_args);
        let mut dest = Client::new(&b);
        for other in [
            b"{bar}:list".as_slice(),
            b"{bar}:hash",
            b"{bar}:zset",
            b"{bar}:vector",
            b"{bar}:stream",
        ] {
            assert_eq!(
                source.call(&[
                    b"MIGRATE",
                    b"127.0.0.1",
                    second_port.to_string().as_bytes(),
                    other,
                    b"0",
                    b"1000"
                ]),
                ok()
            );
        }
        assert_eq!(dest.call(&[b"ASKING"]), ok());
        assert_eq!(
            dest.call(&[b"GET", key]),
            Reply::Bulk(Some(b"\0\xff\r\n".to_vec()))
        );
        assert_eq!(
            dest.call(&[
                b"CLUSTER",
                b"SETSLOT",
                b"5061",
                b"NODE",
                SECOND_ID.as_bytes()
            ]),
            ok()
        );
        assert_eq!(
            source.call(&[
                b"CLUSTER",
                b"SETSLOT",
                b"5061",
                b"NODE",
                SECOND_ID.as_bytes()
            ]),
            ok()
        );
        assert_eq!(
            source.call(&[b"GET", key]),
            Reply::Error(format!("MOVED 5061 {target}").into_bytes())
        );
        for c in [&mut source, &mut dest] {
            let Reply::Array(Some(ranges)) = c.call(&[b"CLUSTER", b"SLOTS"]) else {
                panic!("expected SLOTS array")
            };
            assert_eq!(ranges.len(), 4);
            let Reply::Array(Some(moved)) = &ranges[1] else {
                panic!("expected migrated slot")
            };
            assert_eq!(moved[0], integer(5061));
            assert_eq!(moved[1], integer(5061));
            let Reply::Array(Some(node)) = &moved[2] else {
                panic!("expected node")
            };
            assert_eq!(node[2], Reply::Bulk(Some(SECOND_ID.as_bytes().to_vec())));
        }
        let Reply::Integer(ttl) = dest.call(&[b"PTTL", key]) else {
            panic!("expected TTL")
        };
        assert!((1..=30000).contains(&ttl));
        assert_eq!(
            dest.call(&[b"LRANGE", b"{bar}:list", b"0", b"-1"]),
            array(vec![
                Reply::Bulk(Some(b"a".to_vec())),
                Reply::Bulk(Some(b"b".to_vec()))
            ])
        );
        assert_eq!(
            dest.call(&[b"HGET", b"{bar}:hash", b"f"]),
            Reply::Bulk(Some(b"v".to_vec()))
        );
        assert_eq!(
            dest.call(&[b"ZSCORE", b"{bar}:zset", b"m"]),
            Reply::Bulk(Some(b"1".to_vec()))
        );
        assert_eq!(
            dest.call(&[b"TYPE", b"{bar}:vector"]),
            Reply::Simple(b"vectorset".to_vec())
        );
        assert_eq!(dest.call(&[b"XLEN", b"{bar}:stream"]), integer(1));
    }
    {
        let a = common::Server::start_with(&first_args);
        let b = common::Server::start_with(&second_args);
        let mut source = Client::new(&a);
        let mut dest = Client::new(&b);
        assert_eq!(
            source.call(&[b"GET", key]),
            Reply::Error(format!("MOVED 5061 {target}").into_bytes())
        );
        assert_eq!(
            dest.call(&[b"GET", key]),
            Reply::Bulk(Some(b"\0\xff\r\n".to_vec()))
        );
        assert_eq!(
            dest.call(&[b"HGET", b"{bar}:hash", b"f"]),
            Reply::Bulk(Some(b"v".to_vec()))
        );
        assert_eq!(
            dest.call(&[b"TYPE", b"{bar}:vector"]),
            Reply::Simple(b"vectorset".to_vec())
        );
        assert_eq!(dest.call(&[b"XLEN", b"{bar}:stream"]), integer(1));
    }
    for path in [first_state, second_state, first_data, second_data] {
        let _ = fs::remove_file(path);
    }
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
        left.call(&[b"OBJECT", b"ENCODING", b"foo"]),
        Reply::Error(format!("MOVED 12182 {right_endpoint}").into_bytes())
    );
    assert_eq!(
        right.call(&[b"OBJECT", b"REFCOUNT", b"bar"]),
        Reply::Error(format!("MOVED 5061 {left_endpoint}").into_bytes())
    );
    assert_eq!(
        left.call(&[b"LCS", b"bar", b"foo"]),
        Reply::Error(b"CROSSSLOT Keys in request don't hash to the same slot".to_vec())
    );
    assert!(matches!(
        left.call(&[b"LCS", b"bar", b"{bar}:other"]),
        Reply::Bulk(Some(_))
    ));
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
