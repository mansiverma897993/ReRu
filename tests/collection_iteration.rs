// SPDX-License-Identifier: AGPL-3.0-only
#[path = "common/client.rs"]
mod client;
mod common;

use client::{array, integer, ok, queued, Client, Reply};

fn bulk(bytes: &[u8]) -> Reply {
    Reply::Bulk(Some(bytes.to_vec()))
}

fn scan_page(reply: Reply) -> (Vec<u8>, Vec<Reply>) {
    let Reply::Array(Some(mut parts)) = reply else {
        panic!("scan reply must be an array");
    };
    assert_eq!(parts.len(), 2);
    let Reply::Bulk(Some(cursor)) = parts.remove(0) else {
        panic!("scan cursor must be a bulk string");
    };
    let Reply::Array(Some(values)) = parts.remove(0) else {
        panic!("scan page must be an array");
    };
    (cursor, values)
}

#[test]
fn hash_length_float_increment_and_error_atomicity() {
    let server = common::Server::start();
    let mut c = Client::new(&server);
    assert_eq!(c.call(&[b"HSTRLEN", b"missing", b"f"]), integer(0));
    assert_eq!(c.call(&[b"HSET", b"h", b"f", b"a\0bc"]), integer(1));
    assert_eq!(c.call(&[b"HSTRLEN", b"h", b"f"]), integer(4));
    assert_eq!(c.call(&[b"HSTRLEN", b"h", b"absent"]), integer(0));
    assert_eq!(
        c.call(&[b"HINCRBYFLOAT", b"h", b"price", b"10.5"]),
        bulk(b"10.5")
    );
    assert_eq!(
        c.call(&[b"HINCRBYFLOAT", b"h", b"price", b"-0.5"]),
        bulk(b"10")
    );
    assert_eq!(c.call(&[b"HGET", b"h", b"price"]), bulk(b"10"));
    assert!(matches!(
        c.call(&[b"HINCRBYFLOAT", b"h", b"f", b"1"]),
        Reply::Error(_)
    ));
    assert!(matches!(
        c.call(&[b"HINCRBYFLOAT", b"h", b"price", b"inf"]),
        Reply::Error(_)
    ));
    assert_eq!(c.call(&[b"HGET", b"h", b"price"]), bulk(b"10"));
    assert_eq!(c.call(&[b"MULTI"]), ok());
    assert_eq!(c.call(&[b"HINCRBYFLOAT", b"h", b"price", b"2"]), queued());
    assert_eq!(c.call(&[b"EXEC"]), array(vec![bulk(b"12")]));
    assert_eq!(c.call(&[b"SET", b"string", b"x"]), ok());
    assert!(matches!(
        c.call(&[b"HSTRLEN", b"string", b"f"]),
        Reply::Error(_)
    ));
}

#[test]
fn hash_and_set_scan_filter_after_cursor_page_and_handle_binary_data() {
    let server = common::Server::start();
    let mut c = Client::new(&server);
    assert_eq!(
        c.call(&[b"HSET", b"h", b"a", b"1", b"b\0", b"2", b"c", b"3"]),
        integer(3)
    );
    let (cursor, fields) =
        scan_page(c.call(&[b"HSCAN", b"h", b"0", b"COUNT", b"2", b"MATCH", b"b*"]));
    assert_eq!(cursor, b"2");
    assert_eq!(fields, vec![bulk(b"b\0"), bulk(b"2")]);
    let (cursor, fields) =
        scan_page(c.call(&[b"HSCAN", b"h", &cursor, b"COUNT", b"2", b"NOVALUES"]));
    assert_eq!(cursor, b"0");
    assert_eq!(fields, vec![bulk(b"c")]);
    assert_eq!(
        scan_page(c.call(&[b"HSCAN", b"missing", b"0"])),
        (b"0".to_vec(), vec![])
    );
    assert_eq!(c.call(&[b"SADD", b"s", b"a", b"b\0", b"c"]), integer(3));
    let (cursor, members) =
        scan_page(c.call(&[b"SSCAN", b"s", b"0", b"COUNT", b"1", b"MATCH", b"b*"]));
    assert_eq!(cursor, b"1");
    assert!(members.is_empty());
    let (cursor, members) =
        scan_page(c.call(&[b"SSCAN", b"s", &cursor, b"COUNT", b"2", b"MATCH", b"b*"]));
    assert_eq!(cursor, b"0");
    assert_eq!(members, vec![bulk(b"b\0")]);
    assert!(matches!(c.call(&[b"HSCAN", b"h", b"bad"]), Reply::Error(_)));
    assert!(matches!(
        c.call(&[b"SSCAN", b"s", b"0", b"COUNT", b"0"]),
        Reply::Error(_)
    ));
    assert!(matches!(
        c.call(&[b"SSCAN", b"s", b"0", b"NOVALUES"]),
        Reply::Error(_)
    ));
}

#[test]
fn random_hash_fields_obey_count_and_withvalues() {
    let server = common::Server::start();
    let mut c = Client::new(&server);
    assert_eq!(c.call(&[b"HRANDFIELD", b"missing"]), Reply::Bulk(None));
    assert_eq!(c.call(&[b"HRANDFIELD", b"missing", b"1"]), array(vec![]));
    assert_eq!(c.call(&[b"HSET", b"h", b"a", b"1", b"b", b"2"]), integer(2));
    let field = c.call(&[b"HRANDFIELD", b"h"]);
    assert!(field == bulk(b"a") || field == bulk(b"b"));
    let Reply::Array(Some(distinct)) = c.call(&[b"HRANDFIELD", b"h", b"20"]) else {
        panic!("expected array")
    };
    assert_eq!(distinct.len(), 2);
    assert!(distinct.contains(&bulk(b"a")) && distinct.contains(&bulk(b"b")));
    let Reply::Array(Some(repeating)) = c.call(&[b"HRANDFIELD", b"h", b"-5"]) else {
        panic!("expected array")
    };
    assert_eq!(repeating.len(), 5);
    assert!(repeating
        .iter()
        .all(|field| *field == bulk(b"a") || *field == bulk(b"b")));
    let Reply::Array(Some(pairs)) = c.call(&[b"HRANDFIELD", b"h", b"2", b"WITHVALUES"]) else {
        panic!("expected array")
    };
    assert_eq!(pairs.len(), 4);
    for pair in pairs.as_chunks::<2>().0 {
        assert_eq!(
            pair[1],
            if pair[0] == bulk(b"a") {
                bulk(b"1")
            } else {
                bulk(b"2")
            }
        );
    }
    assert!(matches!(
        c.call(&[b"HRANDFIELD", b"h", b"2", b"BAD"]),
        Reply::Error(_)
    ));
}
