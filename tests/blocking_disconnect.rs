// SPDX-License-Identifier: AGPL-3.0-only
#[path = "common/client.rs"]
#[allow(dead_code)]
mod client;
#[allow(dead_code)]
mod common;

use client::{array, integer, ok, Client, Reply};
use common::Server;
use std::io::Write;
use std::net::{Shutdown, TcpStream};
use std::time::{Duration, Instant};

fn bulk(bytes: &[u8]) -> Reply {
    Reply::Bulk(Some(bytes.to_vec()))
}

fn blocked(server: &Server, args: &[&[u8]]) -> TcpStream {
    let mut socket = server.connect().unwrap();
    let mut request = format!("*{}\r\n", args.len()).into_bytes();
    for arg in args {
        request.extend(format!("${}\r\n", arg.len()).as_bytes());
        request.extend(*arg);
        request.extend(b"\r\n");
    }
    socket.write_all(&request).unwrap();
    std::thread::sleep(Duration::from_millis(40));
    socket
}

fn disconnect(socket: TcpStream) {
    socket.shutdown(Shutdown::Both).unwrap();
    drop(socket);
}

fn wait_for_one_client(observer: &mut Client) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let Reply::Bulk(Some(info)) = observer.call(&[b"INFO", b"clients"]) else {
            panic!("INFO clients did not return a bulk string");
        };
        if info
            .windows(b"connected_clients:1\r\n".len())
            .any(|window| window == b"connected_clients:1\r\n")
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "blocking client threads did not exit"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn disconnected_list_waiters_do_not_steal_pushed_items() {
    let server = Server::start();
    let mut writer = Client::new(&server);

    for command in [
        vec![b"BLPOP".as_slice(), b"queue", b"0"],
        vec![b"BRPOP".as_slice(), b"queue", b"0"],
        vec![b"BLMPOP".as_slice(), b"0", b"1", b"queue", b"LEFT"],
        vec![
            b"BLMOVE".as_slice(),
            b"queue",
            b"destination",
            b"LEFT",
            b"RIGHT",
            b"0",
        ],
    ] {
        disconnect(blocked(&server, &command));
        assert_eq!(writer.call(&[b"RPUSH", b"queue", b"job"]), integer(1));
        assert_eq!(writer.call(&[b"LPOP", b"queue"]), bulk(b"job"));
        assert_eq!(writer.call(&[b"LLEN", b"destination"]), integer(0));
        wait_for_one_client(&mut writer);
    }
}

#[test]
fn disconnected_sorted_waiters_do_not_steal_added_members() {
    let server = Server::start();
    let mut writer = Client::new(&server);
    for command in [
        vec![b"BZPOPMIN".as_slice(), b"queue", b"0"],
        vec![b"BZPOPMAX".as_slice(), b"queue", b"0"],
        vec![b"BZMPOP".as_slice(), b"0", b"1", b"queue", b"MIN"],
    ] {
        disconnect(blocked(&server, &command));
        assert_eq!(writer.call(&[b"ZADD", b"queue", b"1", b"job"]), integer(1));
        assert_eq!(
            writer.call(&[b"ZPOPMIN", b"queue"]),
            array(vec![bulk(b"job"), bulk(b"1")])
        );
        wait_for_one_client(&mut writer);
    }
}

#[test]
fn disconnected_stream_waiters_do_not_claim_new_entries() {
    let server = Server::start();
    let mut writer = Client::new(&server);
    assert_eq!(
        writer.call(&[b"XGROUP", b"CREATE", b"stream", b"group", b"0", b"MKSTREAM"]),
        ok()
    );
    disconnect(blocked(
        &server,
        &[b"XREAD", b"BLOCK", b"0", b"STREAMS", b"stream", b"0-0"],
    ));
    disconnect(blocked(
        &server,
        &[
            b"XREADGROUP",
            b"GROUP",
            b"group",
            b"dead",
            b"BLOCK",
            b"0",
            b"STREAMS",
            b"stream",
            b">",
        ],
    ));
    assert_eq!(
        writer.call(&[b"XADD", b"stream", b"1-0", b"field", b"value"]),
        bulk(b"1-0")
    );
    assert_eq!(
        writer.call(&[b"XREAD", b"STREAMS", b"stream", b"0-0"]),
        array(vec![array(vec![
            bulk(b"stream"),
            array(vec![array(vec![
                bulk(b"1-0"),
                array(vec![bulk(b"field"), bulk(b"value")]),
            ])]),
        ])])
    );
    assert_eq!(
        writer.call(&[
            b"XREADGROUP",
            b"GROUP",
            b"group",
            b"alive",
            b"BLOCK",
            b"200",
            b"STREAMS",
            b"stream",
            b">",
        ]),
        array(vec![array(vec![
            bulk(b"stream"),
            array(vec![array(vec![
                bulk(b"1-0"),
                array(vec![bulk(b"field"), bulk(b"value")]),
            ])]),
        ])])
    );
    wait_for_one_client(&mut writer);
}

#[test]
fn repeated_block_and_disconnect_releases_client_threads() {
    let server = Server::start_with(&["--maxclients", "8"]);
    let mut observer = Client::new(&server);
    for _ in 0..100 {
        disconnect(blocked(&server, &[b"BLPOP", b"missing", b"0"]));
    }
    wait_for_one_client(&mut observer);
    assert_eq!(observer.call(&[b"PING"]), Reply::Simple(b"PONG".to_vec()));
}
