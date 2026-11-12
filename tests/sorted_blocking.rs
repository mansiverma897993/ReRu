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

fn popped(key: &[u8], members: &[(&[u8], &[u8])]) -> Reply {
    array(vec![
        bulk(key),
        array(
            members
                .iter()
                .map(|(member, score)| array(vec![bulk(member), bulk(score)]))
                .collect(),
        ),
    ])
}

#[test]
fn zmpop_prefers_first_nonempty_key_and_preserves_score_order() {
    let server = Server::start();
    let mut c = Client::new(&server);
    assert_eq!(
        c.call(&[b"ZADD", b"z", b"1", b"a", b"2", b"b", b"2", b"c"]),
        integer(3)
    );
    assert_eq!(c.call(&[b"PEXPIRE", b"z", b"60000"]), integer(1));
    assert_eq!(
        c.call(&[b"ZMPOP", b"2", b"missing", b"z", b"MAX", b"COUNT", b"2"]),
        popped(b"z", &[(b"c", b"2"), (b"b", b"2")])
    );
    assert!(matches!(c.call(&[b"PTTL", b"z"]), Reply::Integer(ttl) if ttl > 0));
    assert_eq!(
        c.call(&[b"ZMPOP", b"1", b"z", b"MIN"]),
        popped(b"z", &[(b"a", b"1")])
    );
    assert_eq!(c.call(&[b"TYPE", b"z"]), Reply::Simple(b"none".to_vec()));
    assert_eq!(c.call(&[b"ZMPOP", b"1", b"z", b"MIN"]), Reply::Array(None));
    assert!(matches!(
        c.call(&[b"ZMPOP", b"1", b"z", b"MIN", b"COUNT", b"0"]),
        Reply::Error(_)
    ));
}

#[test]
fn blocking_sorted_pops_wake_on_zadd_and_timeout() {
    let server = Server::start();
    let mut blocked = Client::new(&server);
    let mut writer = Client::new(&server);
    let waiter = std::thread::spawn(move || {
        blocked.call(&[b"BZMPOP", b"2", b"2", b"missing", b"queue", b"MIN"])
    });
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(writer.call(&[b"ZADD", b"queue", b"3", b"job"]), integer(1));
    assert_eq!(waiter.join().unwrap(), popped(b"queue", &[(b"job", b"3")]));
    assert_eq!(
        writer.call(&[b"BZPOPMAX", b"queue", b"0.02"]),
        Reply::Array(None)
    );
    assert_eq!(writer.call(&[b"ZADD", b"queue", b"4", b"next"]), integer(1));
    assert_eq!(
        writer.call(&[b"BZPOPMAX", b"queue", b"1"]),
        array(vec![bulk(b"queue"), bulk(b"next"), bulk(b"4")])
    );
}

#[test]
fn sorted_blocking_commands_return_immediately_in_transactions() {
    let server = Server::start();
    let mut c = Client::new(&server);
    assert_eq!(c.call(&[b"MULTI"]), ok());
    assert_eq!(
        c.call(&[b"BZMPOP", b"0", b"1", b"missing", b"MIN"]),
        queued()
    );
    assert_eq!(c.call(&[b"BZPOPMIN", b"missing", b"0"]), queued());
    assert_eq!(
        c.call(&[b"EXEC"]),
        array(vec![Reply::Array(None), Reply::Array(None)])
    );
    assert_eq!(c.call(&[b"SET", b"wrong", b"text"]), ok());
    assert!(matches!(
        c.call(&[b"ZMPOP", b"1", b"wrong", b"MIN"]),
        Reply::Error(_)
    ));
}
