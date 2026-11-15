// SPDX-License-Identifier: AGPL-3.0-only
#[path = "common/client.rs"]
#[allow(dead_code)]
mod client;
#[allow(dead_code)]
mod common;

use client::{array, integer, ok, Client, Reply};
use common::Server;
use std::time::{Duration, Instant};

fn bulk(value: &[u8]) -> Reply {
    Reply::Bulk(Some(value.to_vec()))
}

fn ack(kind: &[u8], channel: Option<&[u8]>, count: i64) -> Reply {
    array(vec![
        bulk(kind),
        channel.map_or(Reply::Bulk(None), bulk),
        integer(count),
    ])
}

#[test]
fn publishes_binary_messages_to_multiple_subscribers() {
    let server = Server::start();
    let mut publisher = Client::new(&server);
    let mut first = Client::new(&server);
    let mut second = Client::new(&server);
    assert_eq!(
        first.call(&[b"SUBSCRIBE", b"news", b"other"]),
        ack(b"subscribe", Some(b"news"), 1)
    );
    assert_eq!(first.read(), ack(b"subscribe", Some(b"other"), 2));
    assert_eq!(
        second.call(&[b"SUBSCRIBE", b"news"]),
        ack(b"subscribe", Some(b"news"), 1)
    );
    assert_eq!(publisher.call(&[b"PUBLISH", b"news", b"a\0b"]), integer(2));
    let message = array(vec![bulk(b"message"), bulk(b"news"), bulk(b"a\0b")]);
    assert_eq!(first.read(), message);
    assert_eq!(second.read(), message);
    assert_eq!(publisher.call(&[b"PUBLISH", b"missing", b"x"]), integer(0));
    assert_eq!(
        first.call(&[b"PING"]),
        array(vec![bulk(b"pong"), bulk(b"")])
    );
    assert!(matches!(first.call(&[b"GET", b"news"]), Reply::Error(_)));
    assert_eq!(
        first.call(&[b"UNSUBSCRIBE", b"news"]),
        ack(b"unsubscribe", Some(b"news"), 1)
    );
    assert_eq!(publisher.call(&[b"PUBLISH", b"news", b"x"]), integer(1));
    assert_eq!(
        second.read(),
        array(vec![bulk(b"message"), bulk(b"news"), bulk(b"x")])
    );
    assert_eq!(
        first.call(&[b"UNSUBSCRIBE"]),
        ack(b"unsubscribe", Some(b"other"), 0)
    );
    assert_eq!(first.call(&[b"PING"]), Reply::Simple(b"PONG".to_vec()));
}

#[test]
fn pattern_subscriptions_and_introspection() {
    let server = Server::start();
    let mut publisher = Client::new(&server);
    let mut listener = Client::new(&server);
    assert_eq!(
        listener.call(&[b"PSUBSCRIBE", b"cache:*"]),
        ack(b"psubscribe", Some(b"cache:*"), 1)
    );
    assert_eq!(
        listener.call(&[b"SUBSCRIBE", b"cache:a"]),
        ack(b"subscribe", Some(b"cache:a"), 2)
    );
    assert_eq!(
        publisher.call(&[b"PUBSUB", b"CHANNELS"]),
        array(vec![bulk(b"cache:a")])
    );
    assert_eq!(
        publisher.call(&[b"PUBSUB", b"NUMSUB", b"cache:a", b"other"]),
        array(vec![
            bulk(b"cache:a"),
            integer(1),
            bulk(b"other"),
            integer(0)
        ])
    );
    assert_eq!(publisher.call(&[b"PUBSUB", b"NUMPAT"]), integer(1));
    assert_eq!(
        publisher.call(&[b"PUBLISH", b"cache:a", b"hit"]),
        integer(2)
    );
    assert_eq!(
        listener.read(),
        array(vec![bulk(b"message"), bulk(b"cache:a"), bulk(b"hit")])
    );
    assert_eq!(
        listener.read(),
        array(vec![
            bulk(b"pmessage"),
            bulk(b"cache:*"),
            bulk(b"cache:a"),
            bulk(b"hit")
        ])
    );
    assert_eq!(
        listener.call(&[b"PUNSUBSCRIBE"]),
        ack(b"punsubscribe", Some(b"cache:*"), 1)
    );
    assert_eq!(
        listener.call(&[b"UNSUBSCRIBE"]),
        ack(b"unsubscribe", Some(b"cache:a"), 0)
    );
    assert_eq!(publisher.call(&[b"PUBSUB", b"NUMPAT"]), integer(0));
    assert_eq!(publisher.call(&[b"PUBSUB", b"CHANNELS"]), array(vec![]));
    assert_eq!(listener.call(&[b"SET", b"after", b"ok"]), ok());
}

#[test]
fn disconnect_releases_subscriptions() {
    let server = Server::start();
    let mut publisher = Client::new(&server);
    {
        let mut listener = Client::new(&server);
        assert_eq!(
            listener.call(&[b"SUBSCRIBE", b"gone"]),
            ack(b"subscribe", Some(b"gone"), 1)
        );
        assert_eq!(listener.call(&[b"QUIT"]), ok());
    }
    // The listener thread unregisters after it replies to QUIT, so give the
    // cleanup a bounded window before asserting the final state.
    let deadline = Instant::now() + Duration::from_secs(5);
    while publisher.call(&[b"PUBSUB", b"CHANNELS"]) != array(vec![]) {
        assert!(
            Instant::now() < deadline,
            "subscription was not released after the client disconnected"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(publisher.call(&[b"PUBLISH", b"gone", b"x"]), integer(0));
}

fn command(args: &[&[u8]]) -> Vec<u8> {
    let mut bytes = format!("*{}\r\n", args.len()).into_bytes();
    for arg in args {
        bytes.extend(format!("${}\r\n", arg.len()).as_bytes());
        bytes.extend(*arg);
        bytes.extend(b"\r\n");
    }
    bytes
}

#[test]
fn a_stalled_subscriber_does_not_abort_the_server() {
    use std::io::{Read, Write};
    use std::net::TcpStream;

    let server = Server::start();
    let mut stalled = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
    stalled
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    stalled
        .write_all(&command(&[b"SUBSCRIBE", b"stalled"]))
        .unwrap();
    let mut ack = [0u8; 64];
    assert!(stalled.read(&mut ack).unwrap() > 0);

    // This publish cannot drain into the stalled socket, so the hub drops it.
    let mut publisher = Client::new(&server);
    let payload = vec![b'x'; 16 * 1024 * 1024];
    assert_eq!(
        publisher.call(&[b"PUBLISH", b"stalled", &payload]),
        integer(0)
    );

    // The removed subscription must not panic in SUBSCRIBE or in Drop.
    let _ = stalled.write_all(&command(&[b"SUBSCRIBE", b"other"]));
    let mut sink = [0u8; 65536];
    loop {
        match stalled.read(&mut sink) {
            Ok(0) | Err(_) => break,
            Ok(_) => continue,
        }
    }

    let mut survivor = Client::new(&server);
    assert_eq!(survivor.call(&[b"PING"]), Reply::Simple(b"PONG".to_vec()));
}
