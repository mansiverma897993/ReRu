// SPDX-License-Identifier: AGPL-3.0-only
#[path = "common/client.rs"]
#[allow(dead_code)]
mod client;
#[allow(dead_code)]
mod common;

use client::{array, integer, ok, queued, Client, Reply};
use common::Server;

fn bulk(bytes: &[u8]) -> Reply {
    Reply::Bulk(Some(bytes.to_vec()))
}

fn error(message: &str) -> Reply {
    Reply::Error(format!("ERR {message}").into_bytes())
}

#[test]
fn non_canonical_integer_arguments_are_rejected() {
    let server = Server::start();
    let mut c = Client::new(&server);
    let integer_error = || error("value is not an integer or out of range");

    assert_eq!(c.call(&[b"SET", b"k", b"010"]), ok());
    assert_eq!(c.call(&[b"INCRBY", b"k", b"1"]), integer_error());
    assert_eq!(c.call(&[b"INCR", b"k"]), integer_error());
    assert_eq!(c.call(&[b"SET", b"k", b"1"]), ok());
    assert_eq!(c.call(&[b"INCRBY", b"k", b"+3"]), integer_error());
    assert_eq!(c.call(&[b"DECRBY", b"k", b"010"]), integer_error());
    assert_eq!(c.call(&[b"GET", b"k"]), bulk(b"1"));

    assert_eq!(c.call(&[b"SET", b"g", b"value"]), ok());
    assert_eq!(c.call(&[b"GETRANGE", b"g", b"+0", b"-1"]), integer_error());
    assert_eq!(c.call(&[b"SETRANGE", b"g", b"+1", b"x"]), integer_error());
    assert_eq!(c.call(&[b"SETEX", b"g", b"010", b"value"]), integer_error());

    assert_eq!(c.call(&[b"RPUSH", b"l", b"a", b"b"]), integer(2));
    assert_eq!(
        c.call(&[b"LMPOP", b"+1", b"l", b"LEFT"]),
        error("numkeys should be greater than 0")
    );
    assert_eq!(
        c.call(&[b"LMPOP", b"1", b"l", b"LEFT", b"COUNT", b"+1"]),
        error("count should be greater than 0")
    );
    assert_eq!(c.call(&[b"LRANGE", b"l", b"+0", b"-1"]), integer_error());
    assert_eq!(
        c.call(&[b"LPOP", b"l", b"010"]),
        error("value is out of range, must be positive")
    );
    assert_eq!(
        c.call(&[b"LPOS", b"l", b"a", b"RANK", b"+1"]),
        integer_error()
    );

    assert_eq!(c.call(&[b"ZADD", b"z", b"1", b"a"]), integer(1));
    assert_eq!(c.call(&[b"ZRANGE", b"z", b"+0", b"-1"]), integer_error());
    assert_eq!(
        c.call(&[
            b"ZRANGEBYSCORE",
            b"z",
            b"-inf",
            b"+inf",
            b"LIMIT",
            b"+0",
            b"1"
        ]),
        integer_error()
    );
    assert_eq!(
        c.call(&[b"ZUNIONSTORE", b"out", b"+1", b"z"]),
        integer_error()
    );
    assert_eq!(
        c.call(&[b"ZUNIONSTORE", b"out", b"0", b"z"]),
        error("at least 1 input key is needed for 'zunionstore' command")
    );
    assert_eq!(c.call(&[b"ZINTERCARD", b"+1", b"z"]), integer_error());
    assert_eq!(
        c.call(&[b"ZINTERCARD", b"0", b"z"]),
        error("at least 1 input key is needed for 'zintercard' command")
    );

    assert_eq!(c.call(&[b"SADD", b"s", b"a"]), integer(1));
    assert_eq!(
        c.call(&[b"SINTERCARD", b"+1", b"s"]),
        error("numkeys should be greater than 0")
    );
    assert_eq!(
        c.call(&[b"SINTERCARD", b"1", b"s", b"LIMIT", b"+1"]),
        error("LIMIT can't be negative")
    );

    assert_eq!(c.call(&[b"HSET", b"h", b"f", b"v"]), integer(1));
    assert_eq!(
        c.call(&[b"HEXPIRE", b"h", b"0100", b"FIELDS", b"1", b"f"]),
        integer_error()
    );
    assert_eq!(
        c.call(&[b"HSETEX", b"h2", b"EX", b"0100", b"FIELDS", b"1", b"f", b"v"]),
        integer_error()
    );
    assert_eq!(
        c.call(&[b"HSCAN", b"h", b"0", b"COUNT", b"010"]),
        integer_error()
    );

    assert_eq!(c.call(&[b"SELECT", b"+0"]), integer_error());
    assert_eq!(
        c.call(&[b"HELLO", b"+2"]),
        error("Protocol version is not an integer or out of range")
    );

    assert_eq!(c.call(&[b"SETBIT", b"b", b"0", b"1"]), integer(0));
    assert_eq!(c.call(&[b"BITCOUNT", b"b", b"+0", b"0"]), integer_error());
    assert_eq!(c.call(&[b"BITPOS", b"b", b"+0"]), integer_error());
    assert_eq!(
        c.call(&[b"BITPOS", b"b", b"2"]),
        error("The bit argument must be 1 or 0.")
    );
    assert_eq!(
        c.call(&[b"BITFIELD", b"b", b"GET", b"u+8", b"0"]),
        error("Invalid bitfield type. Use something like i16 u8. Note that u64 is not supported but i64 is.")
    );
    assert_eq!(
        c.call(&[b"BITFIELD", b"b", b"GET", b"u8", b"+0"]),
        error("bit offset is not an integer or out of range")
    );

    assert_eq!(c.call(&[b"GEOADD", b"geo", b"1", b"1", b"a"]), integer(1));
    assert_eq!(
        c.call(&[
            b"GEOSEARCH",
            b"geo",
            b"FROMMEMBER",
            b"a",
            b"BYRADIUS",
            b"1",
            b"km",
            b"COUNT",
            b"+1",
        ]),
        integer_error()
    );
    assert_eq!(
        c.call(&[
            b"GEOSEARCH",
            b"geo",
            b"FROMMEMBER",
            b"a",
            b"BYRADIUS",
            b"1",
            b"km",
            b"COUNT",
            b"0",
        ]),
        error("COUNT must be > 0")
    );

    assert!(matches!(
        c.call(&[b"XADD", b"xs", b"1-0", b"f", b"v"]),
        Reply::Bulk(Some(_))
    ));
    assert_eq!(
        c.call(&[b"XADD", b"xs", b"MAXLEN", b"010", b"2-0", b"f", b"v"]),
        integer_error()
    );
    assert_eq!(
        c.call(&[b"XTRIM", b"xs", b"MAXLEN", b"010"]),
        integer_error()
    );
    assert_eq!(c.call(&[b"SCAN", b"0", b"COUNT", b"010"]), integer_error());
    assert_eq!(
        c.call(&[b"ZSCAN", b"z", b"0", b"COUNT", b"010"]),
        integer_error()
    );
    assert_eq!(
        c.call(&[b"LCS", b"k", b"k", b"IDX", b"MINMATCHLEN", b"010"]),
        integer_error()
    );
}

#[test]
fn non_canonical_integers_inside_multi_report_on_exec() {
    let server = Server::start();
    let mut c = Client::new(&server);
    assert_eq!(c.call(&[b"MULTI"]), ok());
    assert_eq!(c.call(&[b"SET", b"a", b"1"]), queued());
    assert_eq!(c.call(&[b"INCRBY", b"a", b"+1"]), queued());
    assert_eq!(
        c.call(&[b"EXEC"]),
        array(vec![ok(), error("value is not an integer or out of range")])
    );
    assert_eq!(c.call(&[b"GET", b"a"]), bulk(b"1"));
}

#[test]
fn non_canonical_protocol_lengths_are_rejected() {
    use std::io::{Read, Write};
    use std::net::TcpStream;
    use std::time::Duration;

    for frame in [
        &b"*+2\r\n$3\r\nGET\r\n$1\r\nk\r\n"[..],
        &b"*02\r\n$3\r\nGET\r\n$1\r\nk\r\n"[..],
        &b"*1\r\n$+5\r\nhello\r\n"[..],
        &b"*1\r\n$05\r\nhello\r\n"[..],
    ] {
        let server = Server::start();
        let mut stream = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        stream.write_all(frame).unwrap();
        let mut reply = Vec::new();
        let mut chunk = [0u8; 1024];
        loop {
            match stream.read(&mut chunk) {
                Ok(0) => break,
                Ok(read) => reply.extend_from_slice(&chunk[..read]),
                Err(error) => panic!("connection did not close: {error}"),
            }
        }
        assert!(
            reply.starts_with(b"-ERR Protocol error: invalid "),
            "{frame:?} -> {reply:?}"
        );
    }
}
