// SPDX-License-Identifier: AGPL-3.0-only
#[path = "common/client.rs"]
#[allow(dead_code)]
mod client;
#[allow(dead_code)]
mod common;

use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream};
use std::time::Duration;

fn send(frame: &[u8]) -> Vec<u8> {
    let server = common::Server::start();
    let mut stream = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    stream.write_all(frame).unwrap();
    // Half-close so the server reaches EOF after the frame.
    stream.shutdown(Shutdown::Write).unwrap();
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => bytes.extend_from_slice(&chunk[..read]),
            Err(error) => panic!("connection did not close: {error}"),
        }
    }
    bytes
}

#[test]
fn malformed_frames_get_a_protocol_error_then_close() {
    for (frame, reply) in [
        (
            &b"*2\r\n$3\r\nGET\r\n$-1\r\n"[..],
            "-ERR Protocol error: invalid bulk length\r\n",
        ),
        (
            &b"*1\r\n+PING\r\n"[..],
            "-ERR Protocol error: expected '$', got '+'\r\n",
        ),
        (
            &b"PING\r\n"[..],
            "-ERR Protocol error: expected '*', got 'P'\r\n",
        ),
        (
            &b"*abc\r\n"[..],
            "-ERR Protocol error: invalid multibulk length\r\n",
        ),
        (
            &b"*999999999999\r\n"[..],
            "-ERR Protocol error: invalid multibulk length\r\n",
        ),
        (
            &[b"*", &[b'9'; 100][..], b"\r\n"].concat(),
            "-ERR Protocol error: invalid multibulk length\r\n",
        ),
    ] {
        assert_eq!(send(frame), reply.as_bytes(), "frame {frame:?}");
    }
}

#[test]
fn truncated_and_empty_frames_behave_like_redis() {
    // A peer that closes mid-frame gets no reply.
    assert_eq!(send(b"*1"), b"");
    assert_eq!(send(b"*1\r\n$4"), b"");
    assert_eq!(send(b""), b"");

    // Empty frames are ignored, including between pipelined commands.
    assert_eq!(send(b"*0\r\n*1\r\n$4\r\nPING\r\n"), b"+PONG\r\n");
    assert_eq!(send(b"*-1\r\n*1\r\n$4\r\nPING\r\n"), b"+PONG\r\n");
    assert_eq!(
        send(b"*0\r\n*-1\r\n*0\r\n*1\r\n$4\r\nPING\r\n"),
        b"+PONG\r\n"
    );
    assert_eq!(
        send(b"*1\r\n$4\r\nPING\r\n*0\r\n*1\r\n$4\r\nPING\r\n"),
        b"+PONG\r\n+PONG\r\n"
    );
}
