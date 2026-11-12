// SPDX-License-Identifier: AGPL-3.0-only
#[path = "common/client.rs"]
#[allow(dead_code)]
mod client;
#[allow(dead_code)]
mod common;

use client::{array, ok, Client, Reply};
use common::Server;
use std::io::{Read, Write};
use std::time::{Duration, Instant};

fn wait_clients(c: &mut Client, count: usize) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let Reply::Bulk(Some(info)) = c.call(&[b"INFO", b"clients"]) else {
            panic!("expected INFO")
        };
        if String::from_utf8(info)
            .unwrap()
            .lines()
            .any(|s| s == format!("connected_clients:{count}"))
        {
            return;
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn connection_limit_rejects_and_releases_slots_on_quit_disconnect_and_bad_resp() {
    let server = Server::start_with(&["--maxclients", "2"]);
    let mut control = Client::new(&server);
    wait_clients(&mut control, 1);
    let mut second = Client::new(&server);
    assert_eq!(second.call(&[b"PING"]), Reply::Simple(b"PONG".to_vec()));
    let mut rejected = Client::new(&server);
    assert_eq!(
        rejected.read(),
        Reply::Error(b"ERR max number of clients reached".to_vec())
    );
    assert_eq!(second.call(&[b"QUIT"]), ok());
    wait_clients(&mut control, 1);
    let mut replacement = Client::new(&server);
    assert_eq!(
        replacement.call(&[b"PING"]),
        Reply::Simple(b"PONG".to_vec())
    );
    drop(replacement);
    wait_clients(&mut control, 1);
    let mut malformed = server.connect().unwrap();
    malformed.write_all(b"*not-a-number\r\n").unwrap();
    let mut buf = [0; 128];
    let _ = malformed.read(&mut buf);
    drop(malformed);
    wait_clients(&mut control, 1);
    let mut final_client = Client::new(&server);
    assert_eq!(
        final_client.call(&[b"PING"]),
        Reply::Simple(b"PONG".to_vec())
    );
    let Reply::Bulk(Some(info)) = control.call(&[b"INFO", b"stats"]) else {
        panic!("expected INFO")
    };
    assert!(String::from_utf8(info)
        .unwrap()
        .contains("rejected_connections:1\r\n"));
}

#[test]
fn runtime_limit_changes_are_atomic_and_do_not_disconnect_existing_clients() {
    let server = Server::start();
    let mut control = Client::new(&server);
    let mut second = Client::new(&server);
    assert_eq!(second.call(&[b"PING"]), Reply::Simple(b"PONG".to_vec()));
    assert_eq!(
        control.call(&[b"CONFIG", b"SET", b"maxclients", b"1"]),
        ok()
    );
    assert_eq!(second.call(&[b"PING"]), Reply::Simple(b"PONG".to_vec()));
    let mut rejected = Client::new(&server);
    assert!(matches!(rejected.read(), Reply::Error(_)));
    assert!(matches!(
        control.call(&[
            b"CONFIG",
            b"SET",
            b"maxclients",
            b"3",
            b"maxmemory-policy",
            b"invalid"
        ]),
        Reply::Error(_)
    ));
    assert_eq!(
        control.call(&[b"CONFIG", b"GET", b"maxclients"]),
        array(vec![
            Reply::Bulk(Some(b"maxclients".to_vec())),
            Reply::Bulk(Some(b"1".to_vec()))
        ])
    );
    assert!(matches!(
        control.call(&[b"CONFIG", b"SET", b"maxclients", b"0"]),
        Reply::Error(_)
    ));
    assert_eq!(
        control.call(&[b"CONFIG", b"SET", b"maxclients", b"3"]),
        ok()
    );
    let mut third = Client::new(&server);
    assert_eq!(third.call(&[b"PING"]), Reply::Simple(b"PONG".to_vec()));
}
