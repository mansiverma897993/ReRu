// SPDX-License-Identifier: AGPL-3.0-only
#[allow(dead_code)]
mod common;

use common::Server;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

fn read_quiet(stream: &mut TcpStream) -> Vec<u8> {
    stream
        .set_read_timeout(Some(Duration::from_millis(200)))
        .unwrap();
    let mut output = Vec::new();
    match stream.read_to_end(&mut output) {
        Ok(_) => {}
        Err(error)
            if error.kind() == std::io::ErrorKind::WouldBlock
                || error.kind() == std::io::ErrorKind::TimedOut => {}
        Err(error) => panic!("socket read failed: {error}"),
    }
    output
}

fn exchange(server: &Server, commands: &[&[&str]]) -> Vec<u8> {
    let mut stream = server.connect().unwrap();
    let mut input = Vec::new();
    for command in commands {
        input.extend(format!("*{}\r\n", command.len()).as_bytes());
        for argument in *command {
            input.extend(format!("${}\r\n{argument}\r\n", argument.len()).as_bytes());
        }
    }
    stream.write_all(&input).unwrap();
    read_quiet(&mut stream)
}

#[test]
fn off_and_skip_keep_writes_and_reply_boundaries_in_order() {
    let server = Server::start();
    assert_eq!(
        exchange(
            &server,
            &[
                &["PING"],
                &["CLIENT", "REPLY", "OFF"],
                &["SET", "reply:key", "one"],
                &["CLIENT", "REPLY", "ON"],
                &["GET", "reply:key"]
            ]
        ),
        b"+PONG\r\n+OK\r\n$3\r\none\r\n"
    );
    assert_eq!(
        exchange(
            &server,
            &[
                &["CLIENT", "REPLY", "SKIP"],
                &["SET", "reply:key", "two"],
                &["GET", "reply:key"]
            ]
        ),
        b"$3\r\ntwo\r\n"
    );
    assert_eq!(
        exchange(
            &server,
            &[
                &["CLIENT", "REPLY", "SKIP"],
                &["CLIENT", "REPLY", "ON"],
                &["PING"]
            ]
        ),
        b"+OK\r\n+PONG\r\n"
    );
    assert_eq!(
        exchange(
            &server,
            &[
                &["CLIENT", "REPLY", "SKIP"],
                &["CLIENT", "REPLY", "SKIP"],
                &["PING"],
                &["PING"]
            ]
        ),
        b"+PONG\r\n"
    );
}

#[test]
fn errors_are_suppressed_only_when_requested_and_state_is_socket_local() {
    let server = Server::start();
    assert_eq!(
        exchange(&server, &[&["CLIENT", "REPLY"], &["CLIENT", "REPLY", "BAD"], &["CLIENT", "REPLY", "OFF", "extra"], &["PING"]]),
        b"-ERR wrong number of arguments for 'client|reply' command\r\n-ERR syntax error\r\n-ERR wrong number of arguments for 'client|reply' command\r\n+PONG\r\n"
    );
    assert_eq!(
        exchange(
            &server,
            &[&["CLIENT", "REPLY", "SKIP"], &["NOEXIST"], &["PING"]]
        ),
        b"+PONG\r\n"
    );
    assert_eq!(
        exchange(&server, &[&["CLIENT", "REPLY", "OFF"], &["PING"]]),
        b""
    );
    assert_eq!(exchange(&server, &[&["PING"]]), b"+PONG\r\n");
    assert_eq!(
        exchange(
            &server,
            &[
                &["MULTI"],
                &["CLIENT", "REPLY"],
                &["EXEC"],
                &["PING"]
            ]
        ),
        b"+OK\r\n-ERR wrong number of arguments for 'client|reply' command\r\n-EXECABORT Transaction discarded because of previous errors.\r\n+PONG\r\n"
    );
    assert_eq!(
        exchange(
            &server,
            &[
                &["MULTI"],
                &["CLIENT", "REPLY", "BAD"],
                &["EXEC"],
                &["PING"]
            ]
        ),
        b"+OK\r\n+QUEUED\r\n*1\r\n-ERR syntax error\r\n+PONG\r\n"
    );
}

#[test]
fn reset_restores_replies_but_respects_a_pending_skip() {
    let server = Server::start();
    assert_eq!(
        exchange(
            &server,
            &[
                &["CLIENT", "REPLY", "OFF"],
                &["PING"],
                &["RESET"],
                &["PING"]
            ]
        ),
        b"+RESET\r\n+PONG\r\n"
    );
    assert_eq!(
        exchange(
            &server,
            &[&["CLIENT", "REPLY", "SKIP"], &["RESET"], &["PING"]]
        ),
        b"+PONG\r\n"
    );
}

#[test]
fn transactions_and_subscribed_clients_preserve_wire_framing() {
    let server = Server::start();
    assert_eq!(
        exchange(
            &server,
            &[
                &["CLIENT", "REPLY", "SKIP"],
                &["MULTI"],
                &["PING"],
                &["EXEC"],
                &["PING"]
            ]
        ),
        b"+QUEUED\r\n*1\r\n+PONG\r\n+PONG\r\n"
    );
    assert_eq!(
        exchange(
            &server,
            &[
                &["MULTI"],
                &["CLIENT", "REPLY", "ON"],
                &["PING"],
                &["EXEC"],
                &["PING"]
            ]
        ),
        b"+OK\r\n+QUEUED\r\n+QUEUED\r\n*2\r\n+OK\r\n+PONG\r\n+PONG\r\n"
    );
    assert_eq!(
        exchange(
            &server,
            &[
                &["MULTI"],
                &["PING"],
                &["CLIENT", "REPLY", "OFF"],
                &["ECHO", "after"],
                &["EXEC"],
                &["PING"],
                &["CLIENT", "REPLY", "ON"],
                &["PING"]
            ]
        ),
        b"+OK\r\n+QUEUED\r\n+QUEUED\r\n+QUEUED\r\n*3\r\n+PONG\r\n+OK\r\n+PONG\r\n"
    );
    assert_eq!(
        exchange(
            &server,
            &[
                &["MULTI"],
                &["PING"],
                &["CLIENT", "REPLY", "SKIP"],
                &["ECHO", "after"],
                &["EXEC"],
                &["PING"],
                &["CLIENT", "REPLY", "ON"],
                &["PING"]
            ]
        ),
        b"+OK\r\n+QUEUED\r\n+QUEUED\r\n+QUEUED\r\n*3\r\n+PONG\r\n$5\r\nafter\r\n+OK\r\n+PONG\r\n"
    );
    assert_eq!(
        exchange(
            &server,
            &[
                &["CLIENT", "REPLY", "OFF"],
                &["MULTI"],
                &["CLIENT", "REPLY", "ON"],
                &["PING"],
                &["EXEC"],
                &["PING"]
            ]
        ),
        b"+OK\r\n+PONG\r\n+PONG\r\n"
    );
    assert_eq!(
        exchange(&server, &[&["SUBSCRIBE", "reply:channel"], &["CLIENT", "REPLY", "OFF"], &["RESET"], &["PING"]]),
        b"*3\r\n$9\r\nsubscribe\r\n$13\r\nreply:channel\r\n:1\r\n-ERR Can't execute 'client|reply': only (P|S)SUBSCRIBE / (P|S)UNSUBSCRIBE / PING / QUIT / RESET are allowed in this context\r\n+RESET\r\n+PONG\r\n"
    );
    assert_eq!(
        exchange(
            &server,
            &[
                &["CLIENT", "REPLY", "OFF"],
                &["SUBSCRIBE", "reply:channel"],
                &["PING"],
                &["RESET"],
                &["PING"]
            ]
        ),
        b"*3\r\n$9\r\nsubscribe\r\n$13\r\nreply:channel\r\n:1\r\n+RESET\r\n+PONG\r\n"
    );
}

#[test]
fn subscribed_acknowledgements_and_messages_bypass_reply_off() {
    let server = Server::start();
    let mut subscriber = server.connect().unwrap();
    subscriber
        .write_all(b"*3\r\n$6\r\nCLIENT\r\n$5\r\nREPLY\r\n$3\r\nOFF\r\n*2\r\n$9\r\nSUBSCRIBE\r\n$13\r\nreply:channel\r\n")
        .unwrap();
    assert_eq!(
        read_quiet(&mut subscriber),
        b"*3\r\n$9\r\nsubscribe\r\n$13\r\nreply:channel\r\n:1\r\n"
    );
    assert_eq!(
        exchange(&server, &[&["PUBLISH", "reply:channel", "message"]]),
        b":1\r\n"
    );
    assert_eq!(
        read_quiet(&mut subscriber),
        b"*3\r\n$7\r\nmessage\r\n$13\r\nreply:channel\r\n$7\r\nmessage\r\n"
    );
}

#[test]
fn skip_is_ignored_while_replies_are_off() {
    let server = Server::start();
    assert_eq!(
        exchange(
            &server,
            &[
                &["CLIENT", "REPLY", "OFF"],
                &["CLIENT", "REPLY", "SKIP"],
                &["SET", "reply:skip-off", "v"],
                &["GET", "reply:skip-off"]
            ]
        ),
        b""
    );
    assert_eq!(
        exchange(
            &server,
            &[
                &["CLIENT", "REPLY", "OFF"],
                &["CLIENT", "REPLY", "SKIP"],
                &["PING"],
                &["PING"],
                &["CLIENT", "REPLY", "ON"],
                &["PING"]
            ]
        ),
        b"+OK\r\n+PONG\r\n"
    );
    assert_eq!(
        exchange(
            &server,
            &[
                &["CLIENT", "REPLY", "OFF"],
                &["CLIENT", "REPLY", "SKIP"],
                &["RESET"],
                &["PING"]
            ]
        ),
        b"+RESET\r\n+PONG\r\n"
    );
    assert_eq!(
        exchange(
            &server,
            &[
                &["MULTI"],
                &["CLIENT", "REPLY", "OFF"],
                &["CLIENT", "REPLY", "SKIP"],
                &["PING"],
                &["EXEC"],
                &["PING"]
            ]
        ),
        b"+OK\r\n+QUEUED\r\n+QUEUED\r\n+QUEUED\r\n*3\r\n"
    );
}
