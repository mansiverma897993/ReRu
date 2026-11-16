// SPDX-License-Identifier: AGPL-3.0-only
#[path = "common/client.rs"]
#[allow(dead_code)]
mod client;
#[allow(dead_code)]
mod common;

use client::{array, integer, ok, queued, Client, Reply};
use common::Server;
use std::time::Duration;

fn bulk(value: &[u8]) -> Reply {
    Reply::Bulk(Some(value.to_vec()))
}

#[test]
fn multi_pop_uses_first_nonempty_key_and_requested_direction() {
    let server = Server::start();
    let mut c = Client::new(&server);
    assert_eq!(c.call(&[b"RPUSH", b"list", b"a", b"b", b"c"]), integer(3));
    assert_eq!(
        c.call(&[b"LMPOP", b"2", b"missing", b"list", b"LEFT", b"COUNT", b"2"]),
        array(vec![bulk(b"list"), array(vec![bulk(b"a"), bulk(b"b")])])
    );
    assert_eq!(
        c.call(&[b"BLMPOP", b"0.1", b"2", b"missing", b"list", b"RIGHT"]),
        array(vec![bulk(b"list"), array(vec![bulk(b"c")])])
    );
    assert_eq!(c.call(&[b"TYPE", b"list"]), Reply::Simple(b"none".to_vec()));
    assert_eq!(
        c.call(&[b"LMPOP", b"1", b"missing", b"LEFT"]),
        Reply::Array(None)
    );
    assert_eq!(
        c.call(&[b"BLMPOP", b"0.02", b"1", b"missing", b"LEFT"]),
        Reply::Array(None)
    );
    assert!(matches!(
        c.call(&[b"LMPOP", b"1", b"missing", b"LEFT", b"COUNT", b"0"]),
        Reply::Error(_)
    ));
}

#[test]
fn blocking_moves_wake_on_push_and_preserve_atomic_destination_updates() {
    let server = Server::start();
    let mut blocked = Client::new(&server);
    let mut writer = Client::new(&server);
    let waiter = std::thread::spawn(move || {
        blocked.call(&[b"BLMOVE", b"queue", b"processing", b"RIGHT", b"LEFT", b"2"])
    });
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(writer.call(&[b"RPUSH", b"queue", b"job"]), integer(1));
    assert_eq!(waiter.join().unwrap(), bulk(b"job"));
    assert_eq!(
        writer.call(&[b"LRANGE", b"processing", b"0", b"-1"]),
        array(vec![bulk(b"job")])
    );
    assert_eq!(
        writer.call(&[b"BRPOPLPUSH", b"queue", b"processing", b"0.02"]),
        Reply::Array(None)
    );
    assert_eq!(
        writer.call(&[
            b"BLMOVE",
            b"queue",
            b"processing",
            b"LEFT",
            b"RIGHT",
            b"0.02"
        ]),
        Reply::Array(None)
    );
    assert_eq!(writer.call(&[b"RPUSH", b"queue", b"second"]), integer(1));
    assert_eq!(
        writer.call(&[b"BRPOPLPUSH", b"queue", b"processing", b"1"]),
        bulk(b"second")
    );
    assert_eq!(
        writer.call(&[b"LRANGE", b"processing", b"0", b"-1"]),
        array(vec![bulk(b"second"), bulk(b"job")])
    );
    assert_eq!(writer.call(&[b"MULTI"]), ok());
    assert_eq!(
        writer.call(&[
            b"BLMOVE",
            b"missing",
            b"processing",
            b"RIGHT",
            b"LEFT",
            b"0"
        ]),
        queued()
    );
    assert_eq!(writer.call(&[b"EXEC"]), array(vec![Reply::Bulk(None)]));
}

#[test]
fn blocked_list_clients_ignore_type_changes_after_the_first_pass() {
    let server = Server::start();
    let mut writer = Client::new(&server);
    let mut blocked = Client::new(&server);
    assert_eq!(writer.call(&[b"DEL", b"watched"]), integer(0));
    let waiter = std::thread::spawn(move || blocked.call(&[b"BLPOP", b"watched", b"2"]));
    std::thread::sleep(Duration::from_millis(80));
    assert_eq!(writer.call(&[b"SET", b"watched", b"string"]), ok());
    std::thread::sleep(Duration::from_millis(120));
    assert_eq!(writer.call(&[b"DEL", b"watched"]), integer(1));
    assert_eq!(writer.call(&[b"RPUSH", b"watched", b"value"]), integer(1));
    assert_eq!(
        waiter.join().unwrap(),
        array(vec![bulk(b"watched"), bulk(b"value")])
    );

    // An initial wrong type still fails immediately.
    assert_eq!(writer.call(&[b"SET", b"wrong", b"string"]), ok());
    assert!(matches!(
        writer.call(&[b"BLPOP", b"wrong", b"1"]),
        Reply::Error(message) if message.starts_with(b"WRONGTYPE")
    ));

    let mut mover = Client::new(&server);
    assert_eq!(writer.call(&[b"DEL", b"source"]), integer(0));
    let moving = std::thread::spawn(move || {
        mover.call(&[b"BLMOVE", b"source", b"dest", b"LEFT", b"RIGHT", b"2"])
    });
    std::thread::sleep(Duration::from_millis(80));
    assert_eq!(writer.call(&[b"SET", b"source", b"string"]), ok());
    std::thread::sleep(Duration::from_millis(120));
    assert_eq!(writer.call(&[b"DEL", b"source"]), integer(1));
    assert_eq!(writer.call(&[b"RPUSH", b"source", b"moved"]), integer(1));
    assert_eq!(moving.join().unwrap(), bulk(b"moved"));
}

#[test]
fn blocked_xread_ignores_a_type_change_after_the_first_pass() {
    let server = Server::start();
    let mut writer = Client::new(&server);
    let mut blocked = Client::new(&server);
    assert_eq!(writer.call(&[b"DEL", b"stream"]), integer(0));
    let waiter = std::thread::spawn(move || {
        blocked.call(&[b"XREAD", b"BLOCK", b"2000", b"STREAMS", b"stream", b"0-0"])
    });
    std::thread::sleep(Duration::from_millis(80));
    assert_eq!(writer.call(&[b"SET", b"stream", b"string"]), ok());
    std::thread::sleep(Duration::from_millis(120));
    assert_eq!(writer.call(&[b"DEL", b"stream"]), integer(1));
    assert!(matches!(
        writer.call(&[b"XADD", b"stream", b"1-0", b"f", b"v"]),
        Reply::Bulk(Some(_))
    ));
    assert_eq!(
        waiter.join().unwrap(),
        array(vec![array(vec![
            bulk(b"stream"),
            array(vec![array(vec![
                bulk(b"1-0"),
                array(vec![bulk(b"f"), bulk(b"v")])
            ])]),
        ])])
    );
}
