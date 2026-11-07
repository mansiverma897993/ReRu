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
fn list_index_set_insert_trim_remove_and_right_pop() {
    let server = Server::start();
    let mut client = Client::new(&server);
    assert_eq!(
        client.call(&[b"RPUSH", b"list", b"a", b"b", b"c"]),
        integer(3)
    );
    assert_eq!(client.call(&[b"LINDEX", b"list", b"-1"]), bulk(b"c"));
    assert_eq!(client.call(&[b"LINDEX", b"list", b"3"]), Reply::Bulk(None));
    assert_eq!(client.call(&[b"LSET", b"list", b"-1", b"z"]), ok());
    assert_eq!(
        client.call(&[b"LINSERT", b"list", b"BEFORE", b"b", b"x"]),
        integer(4)
    );
    assert_eq!(
        client.call(&[b"LINSERT", b"list", b"AFTER", b"b", b"y"]),
        integer(5)
    );
    assert_eq!(
        client.call(&[b"LINSERT", b"list", b"BEFORE", b"missing", b"x"]),
        integer(-1)
    );
    assert_eq!(
        client.call(&[b"LRANGE", b"list", b"0", b"-1"]),
        array(vec![
            bulk(b"a"),
            bulk(b"x"),
            bulk(b"b"),
            bulk(b"y"),
            bulk(b"z")
        ])
    );
    assert_eq!(client.call(&[b"LTRIM", b"list", b"1", b"-2"]), ok());
    assert_eq!(client.call(&[b"LREM", b"list", b"0", b"b"]), integer(1));
    assert_eq!(
        client.call(&[b"RPOP", b"list", b"2"]),
        array(vec![bulk(b"y"), bulk(b"x")])
    );
    assert_eq!(
        client.call(&[b"TYPE", b"list"]),
        Reply::Simple(b"none".to_vec())
    );
    assert_eq!(client.call(&[b"RPOP", b"list"]), Reply::Bulk(None));
}

#[test]
fn list_move_and_conditional_push_preserve_values_and_ttl() {
    let server = Server::start();
    let mut client = Client::new(&server);
    assert_eq!(client.call(&[b"LPUSHX", b"source", b"x"]), integer(0));
    assert_eq!(client.call(&[b"RPUSHX", b"source", b"x"]), integer(0));
    assert_eq!(client.call(&[b"RPUSH", b"source", b"a", b"b"]), integer(2));
    assert_eq!(client.call(&[b"PEXPIRE", b"source", b"60000"]), integer(1));
    assert_eq!(client.call(&[b"LPUSHX", b"source", b"z"]), integer(3));
    assert_eq!(client.call(&[b"RPUSHX", b"source", b"c"]), integer(4));
    assert_eq!(
        client.call(&[b"LMOVE", b"source", b"dest", b"RIGHT", b"LEFT"]),
        bulk(b"c")
    );
    assert_eq!(client.call(&[b"RPOPLPUSH", b"source", b"dest"]), bulk(b"b"));
    assert_eq!(
        client.call(&[b"LMOVE", b"source", b"source", b"LEFT", b"RIGHT"]),
        bulk(b"z")
    );
    assert_eq!(
        client.call(&[b"LRANGE", b"source", b"0", b"-1"]),
        array(vec![bulk(b"a"), bulk(b"z")])
    );
    assert_eq!(
        client.call(&[b"LRANGE", b"dest", b"0", b"-1"]),
        array(vec![bulk(b"b"), bulk(b"c")])
    );
    assert!(matches!(client.call(&[b"PTTL", b"source"]), Reply::Integer(value) if value > 0));
    assert_eq!(client.call(&[b"PTTL", b"dest"]), integer(-1));
}

#[test]
fn list_move_wrong_destination_is_atomic_and_lpos_options_work() {
    let server = Server::start();
    let mut client = Client::new(&server);
    assert_eq!(
        client.call(&[b"RPUSH", b"list", b"a", b"b", b"a", b"a"]),
        integer(4)
    );
    assert_eq!(client.call(&[b"SET", b"string", b"value"]), ok());
    assert!(matches!(
        client.call(&[b"LMOVE", b"list", b"string", b"RIGHT", b"LEFT"]),
        Reply::Error(_)
    ));
    assert_eq!(
        client.call(&[b"LRANGE", b"list", b"0", b"-1"]),
        array(vec![bulk(b"a"), bulk(b"b"), bulk(b"a"), bulk(b"a")])
    );
    assert_eq!(client.call(&[b"LPOS", b"list", b"a"]), integer(0));
    assert_eq!(
        client.call(&[b"LPOS", b"list", b"a", b"RANK", b"2"]),
        integer(2)
    );
    assert_eq!(
        client.call(&[b"LPOS", b"list", b"a", b"RANK", b"-1"]),
        integer(3)
    );
    assert_eq!(
        client.call(&[b"LPOS", b"list", b"a", b"COUNT", b"0"]),
        array(vec![integer(0), integer(2), integer(3)])
    );
    assert_eq!(
        client.call(&[b"LPOS", b"list", b"a", b"MAXLEN", b"2", b"COUNT", b"0"]),
        array(vec![integer(0)])
    );
    assert_eq!(client.call(&[b"LREM", b"list", b"-1", b"a"]), integer(1));
    assert_eq!(
        client.call(&[b"LRANGE", b"list", b"0", b"-1"]),
        array(vec![bulk(b"a"), bulk(b"b"), bulk(b"a")])
    );
}
