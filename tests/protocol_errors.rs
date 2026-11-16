// SPDX-License-Identifier: AGPL-3.0-only
#[path = "common/client.rs"]
#[allow(dead_code)]
mod client;
#[allow(dead_code)]
mod common;

use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

fn connection(port: u16) -> TcpStream {
    let stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    stream
}

fn read_to_end(mut stream: TcpStream) -> Vec<u8> {
    let mut bytes = Vec::new();
    let _ = stream.read_to_end(&mut bytes);
    bytes
}

#[test]
fn malformed_frames_get_a_protocol_error_before_close() {
    let server = common::Server::start();
    for (frame, reason) in [
        (&b"*2\r\n$3\r\nGET\r\n$-1\r\n"[..], "invalid bulk length"),
        (&b"*1\r\n+PING\r\n"[..], "expected '$'"),
        (&b"*999999999999\r\n"[..], "invalid multibulk length"),
        (
            &[b"*", &[b'9'; 100][..], b"\r\n"].concat(),
            "invalid multibulk length",
        ),
    ] {
        let mut stream = connection(server.port);
        stream.write_all(frame).unwrap();
        let reply = read_to_end(stream);
        let text = String::from_utf8_lossy(&reply);
        assert!(
            text.starts_with(&format!("-ERR Protocol error: {reason}")),
            "frame {frame:?} replied {text:?}"
        );
    }
}

#[test]
fn empty_frames_are_ignored_and_pipelining_continues() {
    let server = common::Server::start();
    for frame in [
        &b"*0\r\n*1\r\n$4\r\nPING\r\n"[..],
        &b"*-1\r\n*1\r\n$4\r\nPING\r\n"[..],
    ] {
        let mut stream = connection(server.port);
        stream.write_all(frame).unwrap();
        let mut reply = [0u8; 7];
        stream.read_exact(&mut reply).unwrap();
        assert_eq!(&reply, b"+PONG\r\n");
    }
}
