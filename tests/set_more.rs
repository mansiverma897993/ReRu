// SPDX-License-Identifier: AGPL-3.0-only
#[path = "common/client.rs"]
#[allow(dead_code)]
mod client;
#[allow(dead_code)]
mod common;

use client::{array, integer, ok, Client, Reply};
use common::Server;

fn bulk(value: &[u8]) -> Reply {
    Reply::Bulk(Some(value.to_vec()))
}

#[test]
fn intersection_cardinality_accepts_a_limit_and_validates_key_count() {
    let server = Server::start();
    let mut c = Client::new(&server);
    assert_eq!(
        c.call(&[b"SADD", b"a", b"one", b"two", b"three"]),
        integer(3)
    );
    assert_eq!(c.call(&[b"SADD", b"b", b"two", b"three"]), integer(2));
    assert_eq!(c.call(&[b"SINTERCARD", b"2", b"a", b"b"]), integer(2));
    assert_eq!(
        c.call(&[b"SINTERCARD", b"2", b"a", b"b", b"LIMIT", b"1"]),
        integer(1)
    );
    assert_eq!(
        c.call(&[b"SINTERCARD", b"2", b"a", b"b", b"LIMIT", b"0"]),
        integer(2)
    );
    assert_eq!(c.call(&[b"SINTERCARD", b"2", b"a", b"missing"]), integer(0));
    assert!(matches!(
        c.call(&[b"SINTERCARD", b"0", b"a"]),
        Reply::Error(_)
    ));
    assert!(matches!(
        c.call(&[b"SINTERCARD", b"2", b"a"]),
        Reply::Error(_)
    ));
}

#[test]
fn set_store_commands_replace_destination_and_reset_ttl() {
    let server = Server::start();
    let mut client = Client::new(&server);
    assert_eq!(client.call(&[b"SADD", b"a", b"one", b"two"]), integer(2));
    assert_eq!(client.call(&[b"SADD", b"b", b"two", b"three"]), integer(2));
    assert_eq!(client.call(&[b"SET", b"dest", b"old"]), ok());
    assert_eq!(client.call(&[b"PEXPIRE", b"dest", b"60000"]), integer(1));
    assert_eq!(
        client.call(&[b"SUNIONSTORE", b"dest", b"a", b"b"]),
        integer(3)
    );
    assert_eq!(client.call(&[b"SCARD", b"dest"]), integer(3));
    assert_eq!(client.call(&[b"PTTL", b"dest"]), integer(-1));
    assert_eq!(
        client.call(&[b"SINTERSTORE", b"dest", b"a", b"b"]),
        integer(1)
    );
    assert_eq!(
        client.call(&[b"SMEMBERS", b"dest"]),
        array(vec![bulk(b"two")])
    );
    assert_eq!(
        client.call(&[b"SDIFFSTORE", b"dest", b"a", b"b"]),
        integer(1)
    );
    assert_eq!(
        client.call(&[b"SMEMBERS", b"dest"]),
        array(vec![bulk(b"one")])
    );
    assert_eq!(
        client.call(&[b"SINTERSTORE", b"dest", b"a", b"missing"]),
        integer(0)
    );
    assert_eq!(
        client.call(&[b"TYPE", b"dest"]),
        Reply::Simple(b"none".to_vec())
    );
    assert_eq!(client.call(&[b"SET", b"string", b"x"]), ok());
    assert!(matches!(
        client.call(&[b"SUNIONSTORE", b"dest", b"a", b"string"]),
        Reply::Error(_)
    ));
    assert_eq!(
        client.call(&[b"TYPE", b"dest"]),
        Reply::Simple(b"none".to_vec())
    );
}

#[test]
fn smove_is_atomic_and_preserves_destination_expiry() {
    let server = Server::start();
    let mut client = Client::new(&server);
    assert_eq!(client.call(&[b"SADD", b"source", b"item"]), integer(1));
    assert_eq!(client.call(&[b"SADD", b"dest", b"old"]), integer(1));
    assert_eq!(client.call(&[b"PEXPIRE", b"dest", b"60000"]), integer(1));
    assert_eq!(
        client.call(&[b"SMOVE", b"source", b"dest", b"item"]),
        integer(1)
    );
    assert_eq!(
        client.call(&[b"TYPE", b"source"]),
        Reply::Simple(b"none".to_vec())
    );
    assert_eq!(client.call(&[b"SISMEMBER", b"dest", b"item"]), integer(1));
    assert!(matches!(client.call(&[b"PTTL", b"dest"]), Reply::Integer(value) if value > 0));
    assert_eq!(
        client.call(&[b"SMOVE", b"dest", b"dest", b"item"]),
        integer(1)
    );
    assert_eq!(client.call(&[b"SET", b"wrong", b"x"]), ok());
    assert!(matches!(
        client.call(&[b"SMOVE", b"dest", b"wrong", b"item"]),
        Reply::Error(_)
    ));
    assert_eq!(client.call(&[b"SISMEMBER", b"dest", b"item"]), integer(1));
}

#[test]
fn random_member_and_pop_count_use_expected_reply_shapes() {
    let server = Server::start();
    let mut client = Client::new(&server);
    assert_eq!(
        client.call(&[b"SRANDMEMBER", b"missing"]),
        Reply::Bulk(None)
    );
    assert_eq!(
        client.call(&[b"SRANDMEMBER", b"missing", b"3"]),
        array(vec![])
    );
    assert_eq!(client.call(&[b"SADD", b"s", b"a", b"b", b"c"]), integer(3));
    assert!(
        matches!(client.call(&[b"SRANDMEMBER", b"s"]), Reply::Bulk(Some(member)) if [b"a".as_slice(), b"b", b"c"].contains(&member.as_slice()))
    );
    let Reply::Array(Some(distinct)) = client.call(&[b"SRANDMEMBER", b"s", b"10"]) else {
        panic!("expected an array");
    };
    assert_eq!(distinct.len(), 3);
    let Reply::Array(Some(repeated)) = client.call(&[b"SRANDMEMBER", b"s", b"-5"]) else {
        panic!("expected an array");
    };
    assert_eq!(repeated.len(), 5);
    assert_eq!(client.call(&[b"SCARD", b"s"]), integer(3));
    let Reply::Array(Some(popped)) = client.call(&[b"SPOP", b"s", b"2"]) else {
        panic!("expected an array");
    };
    assert_eq!(popped.len(), 2);
    assert_ne!(popped[0], popped[1]);
    assert_eq!(client.call(&[b"SCARD", b"s"]), integer(1));
    assert!(matches!(
        client.call(&[b"SPOP", b"s"]),
        Reply::Bulk(Some(_))
    ));
    assert_eq!(
        client.call(&[b"TYPE", b"s"]),
        Reply::Simple(b"none".to_vec())
    );
}

#[test]
fn large_positive_counts_clamp_to_the_set_size() {
    let server = Server::start();
    let mut client = Client::new(&server);
    assert_eq!(client.call(&[b"SADD", b"s", b"a", b"b"]), integer(2));
    let Reply::Array(Some(popped)) = client.call(&[b"SPOP", b"s", b"1000001"]) else {
        panic!("expected an array");
    };
    assert_eq!(popped.len(), 2);
    assert_ne!(popped[0], popped[1]);
    assert_eq!(
        client.call(&[b"TYPE", b"s"]),
        Reply::Simple(b"none".to_vec())
    );

    assert_eq!(client.call(&[b"SADD", b"s", b"a", b"b"]), integer(2));
    let Reply::Array(Some(members)) = client.call(&[b"SRANDMEMBER", b"s", b"1000001"]) else {
        panic!("expected an array");
    };
    assert_eq!(members.len(), 2);
    assert_ne!(members[0], members[1]);
    assert!(matches!(
        client.call(&[b"SRANDMEMBER", b"s", b"9223372036854775807"]),
        Reply::Array(Some(rows)) if rows.len() == 2
    ));

    let minimum = Reply::Error(
        b"ERR value is out of range, value must between -9223372036854775807 and 9223372036854775807"
            .to_vec(),
    );
    assert_eq!(
        client.call(&[b"SRANDMEMBER", b"s", b"-9223372036854775808"]),
        minimum
    );
    assert_eq!(
        client.call(&[b"SRANDMEMBER", b"missing", b"-9223372036854775808"]),
        minimum
    );
    assert_eq!(
        client.call(&[b"SRANDMEMBER", b"s", b"-1000001"]),
        Reply::Error(b"ERR count exceeds ReRu limit".to_vec())
    );
    assert_eq!(
        client.call(&[b"SRANDMEMBER", b"s", b"+1"]),
        Reply::Error(b"ERR value is not an integer or out of range".to_vec())
    );

    // Positive counts at or below the size, and repeated sampling, are unchanged.
    assert!(matches!(
        client.call(&[b"SRANDMEMBER", b"s", b"1"]),
        Reply::Array(Some(rows)) if rows.len() == 1
    ));
    assert!(matches!(
        client.call(&[b"SRANDMEMBER", b"s", b"2"]),
        Reply::Array(Some(rows)) if rows.len() == 2
    ));
    assert_eq!(
        client.call(&[b"TYPE", b"s"]),
        Reply::Simple(b"set".to_vec())
    );

    // Missing keys still return an empty array, including under the cap.
    assert_eq!(
        client.call(&[b"SRANDMEMBER", b"missing", b"1000001"]),
        array(vec![])
    );
    assert_eq!(
        client.call(&[b"SRANDMEMBER", b"missing", b"-1000001"]),
        array(vec![])
    );

    // SPOP rejects negative and non-canonical counts with Redis's own text.
    let positive_only = Reply::Error(b"ERR value is out of range, must be positive".to_vec());
    for count in [
        b"-1".as_slice(),
        b"-9223372036854775808",
        b"+1",
        b"01",
        b"abc",
    ] {
        assert_eq!(client.call(&[b"SPOP", b"s", count]), positive_only);
    }

    // A wrong-type key does not win over count validation.
    assert_eq!(client.call(&[b"SET", b"str", b"x"]), ok());
    assert_eq!(
        client.call(&[b"SRANDMEMBER", b"str", b"-9223372036854775808"]),
        minimum
    );
    assert_eq!(
        client.call(&[b"SRANDMEMBER", b"str", b"abc"]),
        Reply::Error(b"ERR value is not an integer or out of range".to_vec())
    );
}
