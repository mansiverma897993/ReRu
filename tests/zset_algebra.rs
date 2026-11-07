// SPDX-License-Identifier: AGPL-3.0-only
#[path = "common/client.rs"]
#[allow(dead_code)]
mod client;
#[allow(dead_code)]
mod common;

use client::{array, integer, ok, Client, Reply};

fn values(items: &[&[u8]]) -> Reply {
    array(
        items
            .iter()
            .map(|item| Reply::Bulk(Some(item.to_vec())))
            .collect(),
    )
}

#[test]
fn union_intersection_difference_and_cardinality_order_scores() {
    let server = common::Server::start();
    let mut c = Client::new(&server);
    assert_eq!(
        c.call(&[b"ZADD", b"a", b"1", b"one", b"2", b"two"]),
        integer(2)
    );
    assert_eq!(
        c.call(&[b"ZADD", b"b", b"3", b"one", b"4", b"three"]),
        integer(2)
    );
    assert_eq!(c.call(&[b"SADD", b"s", b"one", b"x"]), integer(2));
    assert_eq!(
        c.call(&[b"ZUNION", b"2", b"a", b"b", b"WITHSCORES"]),
        values(&[b"two", b"2", b"one", b"4", b"three", b"4"])
    );
    assert_eq!(
        c.call(&[b"ZINTER", b"2", b"a", b"b", b"WITHSCORES"]),
        values(&[b"one", b"4"])
    );
    assert_eq!(c.call(&[b"ZDIFF", b"2", b"a", b"b"]), values(&[b"two"]));
    assert_eq!(c.call(&[b"ZINTERCARD", b"2", b"a", b"b"]), integer(1));
    assert_eq!(
        c.call(&[b"ZINTERCARD", b"2", b"a", b"b", b"LIMIT", b"1"]),
        integer(1)
    );
    assert_eq!(
        c.call(&[b"ZUNION", b"2", b"a", b"s", b"WITHSCORES"]),
        values(&[b"x", b"1", b"one", b"2", b"two", b"2"])
    );
    assert_eq!(
        c.call(&[
            b"ZUNION",
            b"2",
            b"a",
            b"b",
            b"WEIGHTS",
            b"2",
            b"3",
            b"AGGREGATE",
            b"MAX",
            b"WITHSCORES",
        ]),
        values(&[b"two", b"4", b"one", b"9", b"three", b"12"])
    );
    assert_eq!(
        c.call(&[
            b"ZUNION",
            b"2",
            b"a",
            b"b",
            b"WEIGHTS",
            b"2",
            b"3",
            b"AGGREGATE",
            b"COUNT",
            b"WITHSCORES",
        ]),
        values(&[b"two", b"2", b"three", b"3", b"one", b"5"])
    );
}

#[test]
fn stored_algebra_replaces_destination_clears_expiry_and_checks_types() {
    let server = common::Server::start();
    let mut c = Client::new(&server);
    assert_eq!(
        c.call(&[b"ZADD", b"a", b"1", b"one", b"2", b"two"]),
        integer(2)
    );
    assert_eq!(c.call(&[b"ZADD", b"b", b"3", b"one"]), integer(1));
    assert_eq!(c.call(&[b"PEXPIRE", b"a", b"60000"]), integer(1));
    assert_eq!(
        c.call(&[b"ZUNIONSTORE", b"a", b"2", b"a", b"b"]),
        integer(2)
    );
    assert_eq!(c.call(&[b"PTTL", b"a"]), integer(-1));
    assert_eq!(
        c.call(&[b"ZRANGE", b"a", b"0", b"-1", b"WITHSCORES"]),
        values(&[b"two", b"2", b"one", b"4"])
    );
    assert_eq!(
        c.call(&[b"ZINTERSTORE", b"out", b"2", b"a", b"b"]),
        integer(1)
    );
    assert_eq!(
        c.call(&[b"ZSCORE", b"out", b"one"]),
        Reply::Bulk(Some(b"7".to_vec()))
    );
    assert_eq!(
        c.call(&[b"ZDIFFSTORE", b"out", b"2", b"a", b"b"]),
        integer(1)
    );
    assert_eq!(c.call(&[b"ZRANGE", b"out", b"0", b"-1"]), values(&[b"two"]));
    assert_eq!(
        c.call(&[b"ZDIFFSTORE", b"out", b"2", b"b", b"b"]),
        integer(0)
    );
    assert_eq!(c.call(&[b"TYPE", b"out"]), Reply::Simple(b"none".to_vec()));
    assert_eq!(c.call(&[b"SET", b"wrong", b"x"]), ok());
    assert!(matches!(
        c.call(&[b"ZUNIONSTORE", b"a", b"2", b"a", b"wrong"]),
        Reply::Error(_)
    ));
    assert_eq!(
        c.call(&[b"ZSCORE", b"a", b"one"]),
        Reply::Bulk(Some(b"4".to_vec()))
    );
}
