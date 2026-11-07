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
