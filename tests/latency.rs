// SPDX-License-Identifier: AGPL-3.0-only
#[path = "common/client.rs"]
#[allow(dead_code)]
mod client;
#[allow(dead_code)]
mod common;

use client::{array, integer, ok, Client, Reply};
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
fn latency_commands_report_history_and_reset_events() {
    let server = Server::start();
    let mut client = Client::new(&server);
    assert_eq!(
        client.call(&[b"CONFIG", b"SET", b"latency-monitor-threshold", b"1"]),
        ok()
    );

    // A blocking command gives a deterministic sample above the 1 ms threshold.
    assert_eq!(
        client.call(&[b"BLPOP", b"missing", b"0.02"]),
        Reply::Array(None)
    );
    let samples = client.call(&[b"LATENCY", b"HISTORY", b"command"]);
    assert!(matches!(&samples, Reply::Array(Some(events)) if events.len() == 1));

    let latest = client.call(&[b"LATENCY", b"LATEST"]);
    let Reply::Array(Some(events)) = latest else {
        panic!("expected latency events, got {latest:?}");
    };
    let [Reply::Array(Some(fields))] = events.as_slice() else {
        panic!("expected one latency event, got {events:?}");
    };
    assert_eq!(fields.len(), 4);
    assert_eq!(fields[0], bulk(b"command"));
    assert!(matches!(fields[1], Reply::Integer(timestamp) if timestamp > 0));
    assert!(matches!(fields[2], Reply::Integer(duration) if duration >= 1));
    assert_eq!(fields[2], fields[3]);

    let history = client.call(&[b"LATENCY", b"HISTORY", b"command"]);
    let Reply::Array(Some(samples)) = history else {
        panic!("expected history, got {history:?}");
    };
    let [Reply::Array(Some(sample))] = samples.as_slice() else {
        panic!("expected one latency sample, got {samples:?}");
    };
    assert_eq!(sample.len(), 2);
    assert!(matches!(sample[0], Reply::Integer(timestamp) if timestamp > 0));
    assert!(matches!(sample[1], Reply::Integer(duration) if duration >= 1));

    assert_eq!(client.call(&[b"LATENCY", b"RESET", b"command"]), integer(1));
    assert_eq!(client.call(&[b"LATENCY", b"LATEST"]), array(vec![]));
    assert_eq!(client.call(&[b"LATENCY", b"RESET"]), integer(0));
    assert_eq!(
        client.call(&[b"LATENCY", b"HISTORY", b"command"]),
        array(vec![])
    );
}

#[test]
fn latency_monitor_can_be_disabled_after_recording() {
    let server = Server::start();
    let mut client = Client::new(&server);
    assert_eq!(
        client.call(&[b"CONFIG", b"SET", b"latency-monitor-threshold", b"1"]),
        ok()
    );
    assert_eq!(
        client.call(&[b"BLPOP", b"missing", b"0.02"]),
        Reply::Array(None)
    );
    let samples = client.call(&[b"LATENCY", b"HISTORY", b"command"]);
    assert!(matches!(&samples, Reply::Array(Some(events)) if events.len() == 1));
    assert_eq!(
        client.call(&[b"CONFIG", b"SET", b"latency-monitor-threshold", b"0"]),
        ok()
    );
    assert_eq!(
        client.call(&[b"BLPOP", b"another", b"0.02"]),
        Reply::Array(None)
    );
    assert_eq!(client.call(&[b"LATENCY", b"HISTORY", b"command"]), samples);
}
