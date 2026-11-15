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

#[test]
fn slow_commands_record_one_sample_and_disabling_keeps_history() {
    let server = Server::start();
    let mut client = Client::new(&server);

    // Populate the keyspace before monitoring is enabled, so setup adds no samples.
    // Each MSET stays under the parser's 1024 argument limit.
    for batch in 0..40 {
        let mut mset: Vec<Vec<u8>> = vec![b"MSET".to_vec()];
        for index in 0..500 {
            let key = batch * 500 + index;
            mset.push(format!("key:{key}").into_bytes());
            mset.push(b"value".to_vec());
        }
        let mset: Vec<&[u8]> = mset.iter().map(Vec::as_slice).collect();
        assert_eq!(client.call(&mset), ok());
    }

    assert_eq!(
        client.call(&[b"CONFIG", b"SET", b"latency-monitor-threshold", b"1"]),
        ok()
    );

    // KEYS over 20k keys takes well over 1 ms, so it records exactly one spike.
    let keys = client.call(&[b"KEYS", b"*"]);
    assert!(matches!(&keys, Reply::Array(Some(keys)) if keys.len() == 20_000));

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
    let Reply::Array(Some(samples)) = &history else {
        panic!("expected history, got {history:?}");
    };
    let [Reply::Array(Some(sample))] = samples.as_slice() else {
        panic!("expected one latency sample, got {samples:?}");
    };
    assert_eq!(sample.len(), 2);
    assert!(matches!(sample[1], Reply::Integer(duration) if duration >= 1));

    // Disabling keeps the samples already collected and stops recording new ones.
    assert_eq!(
        client.call(&[b"CONFIG", b"SET", b"latency-monitor-threshold", b"0"]),
        ok()
    );
    assert_eq!(client.call(&[b"KEYS", b"*"]), keys);
    assert_eq!(client.call(&[b"LATENCY", b"HISTORY", b"command"]), history);
}
