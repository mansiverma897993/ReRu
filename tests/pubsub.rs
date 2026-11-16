// SPDX-License-Identifier: AGPL-3.0-only
#[path = "common/client.rs"]
#[allow(dead_code)]
mod client;
#[allow(dead_code)]
mod common;

use client::{array, integer, ok, queued, Client, Reply};
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
    // Kernel buffers differ per platform, so publish until the stalled
    // subscriber is actually dropped.
    let mut dropped = false;
    for _ in 0..8 {
        if publisher.call(&[b"PUBLISH", b"stalled", &payload]) == integer(0) {
            dropped = true;
            break;
        }
    }
    assert!(dropped, "publish never dropped the stalled subscriber");

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

#[test]
fn many_acks_from_a_non_reader_do_not_block_other_clients() {
    use std::io::{Read, Write};
    use std::net::TcpStream;

    let server = Server::start();
    let mut stalled = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
    let mut args: Vec<Vec<u8>> = vec![b"SUBSCRIBE".to_vec()];
    for index in 0..1023 {
        args.push(format!("channel-{index}-{}", "x".repeat(60_000)).into_bytes());
    }
    let borrowed: Vec<&[u8]> = args.iter().map(Vec::as_slice).collect();
    stalled.write_all(&command(&borrowed)).unwrap();

    // Wait until the server has started writing acknowledgements, so the PING
    // races the batch instead of the command read.
    stalled
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut first = [0u8; 1];
    assert_eq!(stalled.read(&mut first).unwrap(), 1);

    let mut other = Client::new(&server);
    let started = Instant::now();
    assert_eq!(other.call(&[b"PING"]), Reply::Simple(b"PONG".to_vec()));
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "PING blocked for {:?}",
        started.elapsed()
    );

    // The stalled subscriber is disconnected once the batch budget expires.
    stalled
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    let mut sink = [0u8; 65536];
    loop {
        match stalled.read(&mut sink) {
            Ok(0) => break,
            Ok(_) => continue,
            Err(error) => panic!("stalled subscriber was not dropped: {error}"),
        }
    }
}

#[test]
fn pubsub_numpat_counts_unique_patterns() {
    let server = Server::start();
    let mut first = Client::new(&server);
    let mut second = Client::new(&server);
    assert_eq!(
        first.call(&[b"PSUBSCRIBE", b"shared:*"]),
        ack(b"psubscribe", Some(b"shared:*"), 1)
    );
    assert_eq!(
        second.call(&[b"PSUBSCRIBE", b"shared:*"]),
        ack(b"psubscribe", Some(b"shared:*"), 1)
    );
    assert_eq!(
        Client::new(&server).call(&[b"PUBSUB", b"NUMPAT"]),
        integer(1)
    );
    assert_eq!(
        first.call(&[b"PSUBSCRIBE", b"other:*"]),
        ack(b"psubscribe", Some(b"other:*"), 2)
    );
    assert_eq!(
        Client::new(&server).call(&[b"PUBSUB", b"NUMPAT"]),
        integer(2)
    );
    assert_eq!(
        first.call(&[b"PUNSUBSCRIBE", b"shared:*"]),
        ack(b"punsubscribe", Some(b"shared:*"), 1)
    );
    assert_eq!(
        Client::new(&server).call(&[b"PUBSUB", b"NUMPAT"]),
        integer(2)
    );
    assert_eq!(
        second.call(&[b"PUNSUBSCRIBE", b"shared:*"]),
        ack(b"punsubscribe", Some(b"shared:*"), 0)
    );
    assert_eq!(
        Client::new(&server).call(&[b"PUBSUB", b"NUMPAT"]),
        integer(1)
    );
}

#[test]
fn pubsub_numpat_handles_binary_and_duplicate_patterns() {
    let server = Server::start();
    let mut c = Client::new(&server);
    assert_eq!(
        c.call(&[b"PSUBSCRIBE", b"dup:*", b"dup:*"]),
        ack(b"psubscribe", Some(b"dup:*"), 1)
    );
    assert_eq!(c.read(), ack(b"psubscribe", Some(b"dup:*"), 1));
    assert_eq!(
        c.call(&[b"PSUBSCRIBE", b"bin\0:*\xff"]),
        ack(b"psubscribe", Some(b"bin\0:*\xff"), 2)
    );
    assert_eq!(
        Client::new(&server).call(&[b"PUBSUB", b"NUMPAT"]),
        integer(2)
    );
}

#[test]
fn subscribe_inside_multi_queues_and_enters_subscribed_mode() {
    let server = Server::start();
    let mut publisher = Client::new(&server);
    let mut c = Client::new(&server);
    assert_eq!(c.call(&[b"MULTI"]), ok());
    assert_eq!(c.call(&[b"SET", b"k", b"v"]), queued());
    assert_eq!(c.call(&[b"SUBSCRIBE", b"news"]), queued());
    assert_eq!(
        c.call(&[b"EXEC"]),
        array(vec![ok(), ack(b"subscribe", Some(b"news"), 1)])
    );

    assert_eq!(
        c.call(&[b"GET", b"k"]),
        Reply::Error(
            b"ERR Can't execute 'get': only (P|S)SUBSCRIBE / (P|S)UNSUBSCRIBE / PING / QUIT / RESET are allowed in this context"
                .to_vec()
        )
    );
    assert_eq!(c.call(&[b"PING"]), array(vec![bulk(b"pong"), bulk(b"")]));
    assert_eq!(publisher.call(&[b"PUBLISH", b"news", b"hello"]), integer(1));
    assert_eq!(
        c.read(),
        array(vec![bulk(b"message"), bulk(b"news"), bulk(b"hello")])
    );
    assert_eq!(
        c.call(&[b"UNSUBSCRIBE", b"news"]),
        ack(b"unsubscribe", Some(b"news"), 0)
    );
    assert_eq!(c.call(&[b"PING"]), Reply::Simple(b"PONG".to_vec()));
}

#[test]
fn subscribe_inside_multi_validates_arity_and_discard_clears_it() {
    let server = Server::start();
    let mut c = Client::new(&server);
    assert_eq!(c.call(&[b"MULTI"]), ok());
    assert_eq!(
        c.call(&[b"SUBSCRIBE"]),
        Reply::Error(b"ERR wrong number of arguments for 'subscribe' command".to_vec())
    );
    assert_eq!(
        c.call(&[b"EXEC"]),
        Reply::Error(b"EXECABORT Transaction discarded because of previous errors.".to_vec())
    );

    assert_eq!(c.call(&[b"MULTI"]), ok());
    assert_eq!(c.call(&[b"PSUBSCRIBE", b"cache:*"]), queued());
    assert_eq!(c.call(&[b"DISCARD"]), ok());
    assert_eq!(c.call(&[b"PING"]), Reply::Simple(b"PONG".to_vec()));

    assert_eq!(c.call(&[b"MULTI"]), ok());
    assert_eq!(c.call(&[b"SUBSCRIBE", b"a"]), queued());
    assert_eq!(c.call(&[b"SUBSCRIBE", b"b"]), queued());
    assert_eq!(
        c.call(&[b"EXEC"]),
        array(vec![
            ack(b"subscribe", Some(b"a"), 1),
            ack(b"subscribe", Some(b"b"), 2),
        ])
    );
    assert_eq!(
        c.call(&[b"UNSUBSCRIBE"]),
        ack(b"unsubscribe", Some(b"a"), 1)
    );
    assert_eq!(c.read(), ack(b"unsubscribe", Some(b"b"), 0));
}

#[test]
fn subscription_commands_inside_exec_follow_redis_order() {
    let server = Server::start();

    // A publish to the same client during EXEC arrives after the EXEC reply.
    let mut self_publish = Client::new(&server);
    assert_eq!(self_publish.call(&[b"MULTI"]), ok());
    assert_eq!(self_publish.call(&[b"SUBSCRIBE", b"self"]), queued());
    assert_eq!(self_publish.call(&[b"PUBLISH", b"self", b"hi"]), queued());
    assert_eq!(
        self_publish.call(&[b"EXEC"]),
        array(vec![ack(b"subscribe", Some(b"self"), 1), integer(1)])
    );
    assert_eq!(
        self_publish.read(),
        array(vec![bulk(b"message"), bulk(b"self"), bulk(b"hi")])
    );

    // PING after SUBSCRIBE in the same EXEC uses the subscribed reply shape.
    let mut ping = Client::new(&server);
    assert_eq!(ping.call(&[b"MULTI"]), ok());
    assert_eq!(ping.call(&[b"SUBSCRIBE", b"p1"]), queued());
    assert_eq!(ping.call(&[b"PING"]), queued());
    assert_eq!(
        ping.call(&[b"EXEC"]),
        array(vec![
            ack(b"subscribe", Some(b"p1"), 1),
            array(vec![bulk(b"pong"), bulk(b"")]),
        ])
    );

    // Several channels in one queued command produce one ack each.
    let mut multi = Client::new(&server);
    assert_eq!(multi.call(&[b"MULTI"]), ok());
    assert_eq!(multi.call(&[b"SUBSCRIBE", b"a", b"b"]), queued());
    // Redis declares one EXEC element per queued command but emits one ack
    // frame per channel, so the second frame follows the array.
    assert_eq!(
        multi.call(&[b"EXEC"]),
        array(vec![ack(b"subscribe", Some(b"a"), 1)])
    );
    assert_eq!(multi.read(), ack(b"subscribe", Some(b"b"), 2));
    assert_eq!(
        multi.call(&[b"UNSUBSCRIBE", b"a", b"b"]),
        ack(b"unsubscribe", Some(b"a"), 1)
    );
    assert_eq!(multi.read(), ack(b"unsubscribe", Some(b"b"), 0));
    assert_eq!(multi.call(&[b"PING"]), Reply::Simple(b"PONG".to_vec()));

    // A subscribed client's EXEC is rejected with EXECABORT.
    assert_eq!(
        multi.call(&[b"SUBSCRIBE", b"g"]),
        ack(b"subscribe", Some(b"g"), 1)
    );
    assert_eq!(
        multi.call(&[b"EXEC"]),
        Reply::Error(
            b"EXECABORT Transaction discarded because of: Can't execute 'exec': only (P|S)SUBSCRIBE / (P|S)UNSUBSCRIBE / PING / QUIT / RESET are allowed in this context"
                .to_vec()
        )
    );
    assert_eq!(multi.call(&[b"RESET"]), Reply::Simple(b"RESET".to_vec()));
}

#[test]
fn a_stalled_exec_reply_drops_the_client_without_blocking_others() {
    use std::io::{Read, Write};
    use std::net::TcpStream;

    let server = Server::start();
    let mut stalled = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
    let mut subscribe: Vec<Vec<u8>> = vec![b"SUBSCRIBE".to_vec()];
    for index in 0..1023 {
        subscribe.push(format!("channel-{index}-{}", "x".repeat(60_000)).into_bytes());
    }
    let borrowed: Vec<&[u8]> = subscribe.iter().map(Vec::as_slice).collect();
    stalled.write_all(&command(&[b"MULTI"])).unwrap();
    stalled.write_all(&command(&borrowed)).unwrap();
    stalled.write_all(&command(&[b"EXEC"])).unwrap();

    stalled
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut first = [0u8; 1];
    assert_eq!(stalled.read(&mut first).unwrap(), 1);
    std::thread::sleep(Duration::from_millis(1200));

    let mut other = Client::new(&server);
    let started = Instant::now();
    assert_eq!(other.call(&[b"PING"]), Reply::Simple(b"PONG".to_vec()));
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "PING blocked for {:?}",
        started.elapsed()
    );

    // Windows can buffer the whole batch, so the drop is only asserted where
    // socket write timeouts reliably stop a blocked send.
    #[cfg(not(windows))]
    {
        stalled
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut sink = [0u8; 65536];
        loop {
            match stalled.read(&mut sink) {
                Ok(0) => break,
                Ok(_) => continue,
                Err(error) => panic!("stalled EXEC client was not dropped: {error}"),
            }
        }
    }
    #[cfg(windows)]
    let _ = &stalled;
}
