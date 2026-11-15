// SPDX-License-Identifier: AGPL-3.0-only
#[path = "common/client.rs"]
#[allow(dead_code)]
mod client;
#[allow(dead_code)]
mod common;

use client::{array, ok, Client, Reply};
use common::Server;

fn bulk(value: &[u8]) -> Reply {
    Reply::Bulk(Some(value.to_vec()))
}

#[test]
fn latency_monitor_is_disabled_by_default_and_can_be_configured() {
    let server = Server::start();
    let mut client = Client::new(&server);

    assert_eq!(
        client.call(&[b"CONFIG", b"GET", b"latency-monitor-threshold"]),
        array(vec![bulk(b"latency-monitor-threshold"), bulk(b"0")])
    );
    // A slow command still creates no sample while monitoring is disabled.
    assert_eq!(
        client.call(&[b"BLPOP", b"missing", b"0.02"]),
        Reply::Array(None)
    );
    assert_eq!(client.call(&[b"LATENCY", b"LATEST"]), array(vec![]));

    assert_eq!(
        client.call(&[b"CONFIG", b"SET", b"latency-monitor-threshold", b"1"]),
        ok()
    );
    assert_eq!(
        client.call(&[b"CONFIG", b"GET", b"latency-monitor-threshold"]),
        array(vec![bulk(b"latency-monitor-threshold"), bulk(b"1")])
    );
    assert!(matches!(
        client.call(&[b"CONFIG", b"SET", b"latency-monitor-threshold", b"-1"]),
        Reply::Error(_)
    ));
    assert!(matches!(
        client.call(&[b"CONFIG", b"SET", b"latency-monitor-threshold", b"fast"]),
        Reply::Error(_)
    ));
}

#[test]
fn blocking_waits_are_not_reported_as_command_latency() {
    let server = Server::start();
    let mut client = Client::new(&server);
    assert_eq!(
        client.call(&[b"CONFIG", b"SET", b"latency-monitor-threshold", b"1"]),
        ok()
    );

    // Time waiting for data is not command execution latency.
    assert_eq!(
        client.call(&[b"BLPOP", b"missing", b"0.02"]),
        Reply::Array(None)
    );
    assert_eq!(
        client.call(&[b"LATENCY", b"HISTORY", b"command"]),
        array(vec![])
    );
    assert_eq!(client.call(&[b"LATENCY", b"LATEST"]), array(vec![]));
    assert_eq!(
        client.call(&[b"LATENCY", b"LATEST", b"extra"]),
        Reply::Error(b"ERR wrong number of arguments for 'latency|latest' command".to_vec())
    );
    assert_eq!(
        client.call(&[b"LATENCY", b"BOGUS"]),
        Reply::Error(b"ERR unknown subcommand 'BOGUS'. Try LATENCY HELP.".to_vec())
    );
    assert_eq!(
        client.call(&[b"LATENCY", b"HISTORY", b"command"]),
        array(vec![])
    );
}
