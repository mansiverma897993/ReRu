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

    // Repeated publishes fill the 32 MiB output buffer, so the hub drops it.
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

    let mut other = Client::new(&server);
    let started = Instant::now();
    assert_eq!(other.call(&[b"PING"]), Reply::Simple(b"PONG".to_vec()));
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "PING blocked for {:?}",
        started.elapsed()
    );

    // While the batch is buffered, other clients keep working. A client that
    // drains fast keeps its subscription; a stalled one is cut off at the cap.
    stalled
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut sink = [0u8; 65536];
    let mut received = 0usize;
    let mut closed = false;
    loop {
        match stalled.read(&mut sink) {
            Ok(0) => {
                closed = true;
                break;
            }
            Ok(read) => received += read,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => break,
            Err(error) => panic!("stalled subscriber failed: {error}"),
        }
    }
    assert!(received > 0);
    assert_eq!(other.call(&[b"PING"]), Reply::Simple(b"PONG".to_vec()));
    if closed {
        assert!(received < 61 * 1024 * 1024);
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

    // PING keeps its arity check inside a subscribed EXEC.
    let mut bad_ping = Client::new(&server);
    assert_eq!(bad_ping.call(&[b"MULTI"]), ok());
    assert_eq!(bad_ping.call(&[b"SUBSCRIBE", b"q1"]), queued());
    assert_eq!(bad_ping.call(&[b"PING", b"x", b"y"]), queued());
    assert_eq!(
        bad_ping.call(&[b"EXEC"]),
        array(vec![
            ack(b"subscribe", Some(b"q1"), 1),
            Reply::Error(b"ERR wrong number of arguments for 'ping' command".to_vec()),
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

#[test]
fn numpat_tracks_pattern_reference_counts() {
    let server = Server::start();
    let mut publisher = Client::new(&server);
    let mut first = Client::new(&server);
    let mut second = Client::new(&server);

    assert_eq!(
        first.call(&[b"PSUBSCRIBE", b"p\0*"]),
        ack(b"psubscribe", Some(b"p\0*"), 1)
    );
    // Duplicate subscriptions do not increase the distinct count.
    assert_eq!(
        first.call(&[b"PSUBSCRIBE", b"p\0*"]),
        ack(b"psubscribe", Some(b"p\0*"), 1)
    );
    assert_eq!(publisher.call(&[b"PUBSUB", b"NUMPAT"]), integer(1));
    assert_eq!(
        second.call(&[b"PSUBSCRIBE", b"p\0*"]),
        ack(b"psubscribe", Some(b"p\0*"), 1)
    );
    assert_eq!(publisher.call(&[b"PUBSUB", b"NUMPAT"]), integer(1));
    assert_eq!(
        second.call(&[b"PUNSUBSCRIBE", b"p\0*"]),
        ack(b"punsubscribe", Some(b"p\0*"), 0)
    );
    assert_eq!(publisher.call(&[b"PUBSUB", b"NUMPAT"]), integer(1));
    assert_eq!(first.call(&[b"RESET"]), Reply::Simple(b"RESET".to_vec()));
    assert_eq!(publisher.call(&[b"PUBSUB", b"NUMPAT"]), integer(0));

    // The empty pattern is a distinct subscription.
    assert_eq!(
        first.call(&[b"PSUBSCRIBE", b""]),
        ack(b"psubscribe", Some(b""), 1)
    );
    assert_eq!(publisher.call(&[b"PUBSUB", b"NUMPAT"]), integer(1));
    assert_eq!(first.call(&[b"RESET"]), Reply::Simple(b"RESET".to_vec()));
    assert_eq!(publisher.call(&[b"PUBSUB", b"NUMPAT"]), integer(0));

    // A disconnect decrements the pattern reference.
    assert_eq!(
        second.call(&[b"PSUBSCRIBE", b"gone:*"]),
        ack(b"psubscribe", Some(b"gone:*"), 1)
    );
    assert_eq!(publisher.call(&[b"PUBSUB", b"NUMPAT"]), integer(1));
    drop(second);
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if publisher.call(&[b"PUBSUB", b"NUMPAT"]) == integer(0) {
            break;
        }
        assert!(Instant::now() < deadline, "pattern count did not drop");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn numpat_drops_patterns_when_a_stalled_subscriber_is_removed() {
    use std::io::{Read, Write};
    use std::net::TcpStream;

    let server = Server::start();
    let mut publisher = Client::new(&server);
    let mut stalled = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
    stalled
        .write_all(&command(&[b"PSUBSCRIBE", b"dropped:*", b"kept:*"]))
        .unwrap();
    stalled
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut first = [0u8; 1];
    assert_eq!(stalled.read(&mut first).unwrap(), 1);
    assert_eq!(publisher.call(&[b"PUBSUB", b"NUMPAT"]), integer(2));

    // A large message to a non-reading pattern subscriber makes PUBLISH drop it.
    let payload = vec![b'x'; 4 * 1024 * 1024];
    let deadline = Instant::now() + Duration::from_secs(10);
    while publisher.call(&[b"PUBLISH", b"dropped:x", &payload]) != integer(0) {
        assert!(Instant::now() < deadline, "subscriber was not dropped");
    }
    assert_eq!(publisher.call(&[b"PUBSUB", b"NUMPAT"]), integer(0));
    assert_eq!(
        publisher.call(&[b"PUBSUB", b"NUMSUB", b"dropped:x"]),
        array(vec![bulk(b"dropped:x"), integer(0)])
    );
}

fn read_exact_bytes(stream: &mut std::net::TcpStream, length: usize) -> Vec<u8> {
    use std::io::Read;
    let mut bytes = vec![0u8; length];
    let mut offset = 0;
    while offset < length {
        let read = stream.read(&mut bytes[offset..]).unwrap();
        assert!(read > 0, "connection closed after {offset} bytes");
        offset += read;
    }
    bytes
}

#[test]
fn a_paused_subscriber_receives_the_whole_message() {
    use std::io::Write;
    use std::net::TcpStream;

    let server = Server::start();
    let mut subscriber = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
    subscriber
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    subscriber
        .write_all(&command(&[b"SUBSCRIBE", b"news"]))
        .unwrap();
    // *3, $9 subscribe, $4 news, :1
    let ack = read_exact_bytes(&mut subscriber, 33);
    assert_eq!(ack, b"*3\r\n$9\r\nsubscribe\r\n$4\r\nnews\r\n:1\r\n");

    let mut publisher = Client::new(&server);
    let payload = vec![b'x'; 4 * 1024 * 1024];
    // Pause longer than the old 20 ms publish budget while the message is queued.
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(publisher.call(&[b"PUBLISH", b"news", &payload]), integer(1));
    std::thread::sleep(Duration::from_secs(2));

    let mut expected = b"*3\r\n$7\r\nmessage\r\n$4\r\nnews\r\n".to_vec();
    expected.extend(format!("${}\r\n", payload.len()).as_bytes());
    expected.extend_from_slice(&payload);
    expected.extend_from_slice(b"\r\n");
    let frame = read_exact_bytes(&mut subscriber, expected.len());
    assert_eq!(frame, expected);

    // The connection stays usable for further messages.
    assert_eq!(publisher.call(&[b"PUBLISH", b"news", b"after"]), integer(1));
    let next = b"*3\r\n$7\r\nmessage\r\n$4\r\nnews\r\n$5\r\nafter\r\n".to_vec();
    assert_eq!(read_exact_bytes(&mut subscriber, next.len()), next);
}

#[test]
fn a_large_subscription_batch_survives_a_pause() {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpStream;

    fn line(reader: &mut BufReader<TcpStream>) -> Vec<u8> {
        let mut bytes = Vec::new();
        reader.read_until(b'\n', &mut bytes).unwrap();
        assert!(bytes.ends_with(b"\r\n"), "invalid line: {bytes:?}");
        bytes.truncate(bytes.len() - 2);
        bytes
    }

    let server = Server::start();
    let channels: Vec<Vec<u8>> = (0..40)
        .map(|index| format!("{index:02}-{}", "c".repeat(300 * 1024)).into_bytes())
        .collect();

    // Send every subscription without reading, then pause: the whole batch of
    // acknowledgements (>8 MiB) must be buffered instead of truncated.
    let mut subscriber = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
    subscriber
        .set_read_timeout(Some(Duration::from_secs(15)))
        .unwrap();
    for channel in &channels {
        subscriber
            .write_all(&command(&[b"SUBSCRIBE", channel]))
            .unwrap();
    }
    std::thread::sleep(Duration::from_millis(1500));

    let mut reader = BufReader::new(subscriber);
    let mut received: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    for _ in 0..channels.len() {
        assert_eq!(line(&mut reader), b"*3");
        assert_eq!(line(&mut reader), b"$9");
        let mut kind = [0u8; 9];
        reader.read_exact(&mut kind).unwrap();
        assert_eq!(&kind, b"subscribe");
        assert_eq!(line(&mut reader), b"");
        let length = line(&mut reader);
        let length: usize = std::str::from_utf8(&length[1..]).unwrap().parse().unwrap();
        let mut channel = vec![0u8; length];
        reader.read_exact(&mut channel).unwrap();
        let mut suffix = [0u8; 2];
        reader.read_exact(&mut suffix).unwrap();
        assert_eq!(&suffix, b"\r\n");
        received.push((channel, line(&mut reader)));
    }
    let got: Vec<&[u8]> = received.iter().map(|(channel, _)| &channel[..3]).collect();
    let want: Vec<&[u8]> = channels.iter().map(|channel| &channel[..3]).collect();
    if received
        .iter()
        .zip(&channels)
        .any(|((channel, _), expected)| channel != expected)
    {
        panic!("channels {:?} != {:?}", got, want);
    }
    for (index, (_, count)) in received.iter().enumerate() {
        assert_eq!(
            std::str::from_utf8(count).unwrap(),
            format!(":{}", index + 1)
        );
    }

    let mut observer = Client::new(&server);
    assert_eq!(
        observer.call(&[b"PUBSUB", b"NUMSUB", channels[0].as_slice()]),
        array(vec![bulk(&channels[0]), integer(1)])
    );
    assert_eq!(
        observer.call(&[b"PUBLISH", channels[0].as_slice(), b"x"]),
        integer(1)
    );
}
