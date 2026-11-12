// SPDX-License-Identifier: AGPL-3.0-only
#[path = "common/client.rs"]
mod client;
mod common;
use client::{array, integer, ok, queued, Client, Reply};
use std::time::{Duration, Instant};

fn bulk(bytes: &[u8]) -> Reply {
    Reply::Bulk(Some(bytes.to_vec()))
}

#[test]
fn setbit_grows_binary_strings_and_preserves_key_expiry() {
    let server = common::Server::start();
    let mut c = Client::new(&server);
    assert_eq!(c.call(&[b"GETBIT", b"bits", b"0"]), integer(0));
    assert_eq!(c.call(&[b"SET", b"bits", b"A"]), ok());
    assert_eq!(c.call(&[b"PEXPIRE", b"bits", b"10000"]), integer(1));
    assert_eq!(c.call(&[b"SETBIT", b"bits", b"0", b"1"]), integer(0));
    assert_eq!(c.call(&[b"SETBIT", b"bits", b"15", b"1"]), integer(0));
    assert_eq!(c.call(&[b"SETBIT", b"bits", b"15", b"0"]), integer(1));
    assert_eq!(c.call(&[b"GET", b"bits"]), bulk(&[0xc1, 0]));
    assert_eq!(
        c.call(&[b"TYPE", b"bits"]),
        Reply::Simple(b"string".to_vec())
    );
    assert_eq!(c.call(&[b"GETBIT", b"bits", b"4294967295"]), integer(0));
    let Reply::Integer(ttl) = c.call(&[b"PTTL", b"bits"]) else {
        panic!("expected TTL")
    };
    assert!((9000..=10000).contains(&ttl));
}

#[test]
fn bitcount_supports_byte_and_bit_ranges_with_negative_indexes() {
    let server = common::Server::start();
    let mut c = Client::new(&server);
    assert_eq!(c.call(&[b"SET", b"bits", &[0xc1, 0x01]]), ok());
    assert_eq!(c.call(&[b"BITCOUNT", b"bits"]), integer(4));
    assert_eq!(c.call(&[b"BITCOUNT", b"bits", b"0", b"0"]), integer(3));
    assert_eq!(
        c.call(&[b"BITCOUNT", b"bits", b"-1", b"-1", b"BYTE"]),
        integer(1)
    );
    assert_eq!(
        c.call(&[b"BITCOUNT", b"bits", b"0", b"7", b"BIT"]),
        integer(3)
    );
    assert_eq!(
        c.call(&[b"BITCOUNT", b"bits", b"8", b"15", b"BIT"]),
        integer(1)
    );
    assert_eq!(
        c.call(&[b"BITCOUNT", b"bits", b"-8", b"-1", b"BIT"]),
        integer(1)
    );
    assert_eq!(c.call(&[b"BITCOUNT", b"bits", b"3", b"9"]), integer(0));
    assert_eq!(c.call(&[b"BITCOUNT", b"missing"]), integer(0));
}

#[test]
fn bitop_handles_all_operations_missing_inputs_and_destination_overlap() {
    let server = common::Server::start();
    let mut c = Client::new(&server);
    assert_eq!(c.call(&[b"SET", b"a", &[0xf0, 0x0f]]), ok());
    assert_eq!(c.call(&[b"SET", b"b", &[0x0f]]), ok());
    assert_eq!(c.call(&[b"BITOP", b"AND", b"and", b"a", b"b"]), integer(2));
    assert_eq!(c.call(&[b"GET", b"and"]), bulk(&[0, 0]));
    assert_eq!(c.call(&[b"BITOP", b"OR", b"or", b"a", b"b"]), integer(2));
    assert_eq!(c.call(&[b"GET", b"or"]), bulk(&[0xff, 0x0f]));
    assert_eq!(c.call(&[b"BITOP", b"XOR", b"a", b"a", b"b"]), integer(2));
    assert_eq!(c.call(&[b"GET", b"a"]), bulk(&[0xff, 0x0f]));
    assert_eq!(c.call(&[b"BITOP", b"NOT", b"a", b"a"]), integer(2));
    assert_eq!(c.call(&[b"GET", b"a"]), bulk(&[0, 0xf0]));
    assert_eq!(c.call(&[b"SET", b"remove", b"old"]), ok());
    assert_eq!(
        c.call(&[b"BITOP", b"OR", b"remove", b"missing"]),
        integer(0)
    );
    assert_eq!(c.call(&[b"EXISTS", b"remove"]), integer(0));
}

#[test]
fn bitpos_finds_most_significant_bit_and_handles_implicit_zero_padding() {
    let server = common::Server::start();
    let mut c = Client::new(&server);
    assert_eq!(c.call(&[b"BITPOS", b"missing", b"0"]), integer(0));
    assert_eq!(c.call(&[b"BITPOS", b"missing", b"1"]), integer(-1));
    assert_eq!(c.call(&[b"SET", b"bits", &[0xff, 0x7f]]), ok());
    assert_eq!(c.call(&[b"BITPOS", b"bits", b"0"]), integer(8));
    assert_eq!(c.call(&[b"BITPOS", b"bits", b"1"]), integer(0));
    assert_eq!(c.call(&[b"BITPOS", b"bits", b"0", b"0", b"0"]), integer(-1));
    assert_eq!(
        c.call(&[b"BITPOS", b"bits", b"0", b"1", b"1", b"BYTE"]),
        integer(8)
    );
    assert_eq!(
        c.call(&[b"BITPOS", b"bits", b"0", b"0", b"7", b"BIT"]),
        integer(-1)
    );
    assert_eq!(c.call(&[b"SET", b"all", &[0xff]]), ok());
    assert_eq!(c.call(&[b"BITPOS", b"all", b"0"]), integer(8));
    assert_eq!(c.call(&[b"BITPOS", b"all", b"0", b"0"]), integer(8));
    assert_eq!(c.call(&[b"BITPOS", b"all", b"0", b"0", b"0"]), integer(-1));
}

#[test]
fn invalid_bitmap_inputs_and_wrong_types_leave_keys_untouched() {
    let server = common::Server::start();
    let mut c = Client::new(&server);
    assert_eq!(c.call(&[b"RPUSH", b"list", b"x"]), integer(1));
    for args in [
        &[b"GETBIT".as_slice(), b"list", b"1"][..],
        &[b"SETBIT".as_slice(), b"list", b"1", b"1"][..],
        &[b"BITCOUNT".as_slice(), b"list"][..],
        &[b"BITPOS".as_slice(), b"list", b"0"][..],
        &[b"BITOP".as_slice(), b"OR", b"dest", b"list"][..],
    ] {
        assert!(matches!(c.call(args), Reply::Error(_)));
    }
    for args in [
        &[b"SETBIT".as_slice(), b"missing", b"-1", b"1"][..],
        &[b"SETBIT".as_slice(), b"missing", b"536870912", b"1"][..],
        &[b"SETBIT".as_slice(), b"missing", b"0", b"2"][..],
        &[b"BITOP".as_slice(), b"NOT", b"missing", b"a", b"b"][..],
        &[b"BITCOUNT".as_slice(), b"missing", b"1", b"not-an-index"][..],
    ] {
        assert!(matches!(c.call(args), Reply::Error(_)));
    }
    assert_eq!(c.call(&[b"EXISTS", b"missing", b"dest"]), integer(0));
}

#[test]
fn bitmap_commands_in_transactions_update_watches() {
    let server = common::Server::start();
    let mut watcher = Client::new(&server);
    let mut writer = Client::new(&server);
    assert_eq!(writer.call(&[b"SETBIT", b"bitmap", b"0", b"1"]), integer(0));
    assert_eq!(watcher.call(&[b"WATCH", b"bitmap"]), ok());
    assert_eq!(writer.call(&[b"GETBIT", b"bitmap", b"0"]), integer(1));
    assert_eq!(watcher.call(&[b"MULTI"]), ok());
    assert_eq!(watcher.call(&[b"EXEC"]), array(vec![]));
    assert_eq!(watcher.call(&[b"WATCH", b"bitmap"]), ok());
    assert_eq!(writer.call(&[b"SETBIT", b"bitmap", b"0", b"1"]), integer(1));
    assert_eq!(watcher.call(&[b"MULTI"]), ok());
    assert_eq!(watcher.call(&[b"EXEC"]), Reply::Array(None));
    assert_eq!(writer.call(&[b"MULTI"]), ok());
    assert_eq!(writer.call(&[b"SETBIT", b"bitmap", b"1", b"1"]), queued());
    assert_eq!(writer.call(&[b"BITCOUNT", b"bitmap"]), queued());
    assert_eq!(writer.call(&[b"EXEC"]), array(vec![integer(0), integer(2)]));
}

fn wait_for(client: &mut Client, command: &[&[u8]], expected: Reply) {
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if client.call(command) == expected {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("replica did not receive value");
}

#[test]
fn sorted_sets_and_bitmaps_survive_restart_and_replicate() {
    let directory = common::temp_path("reru-sorted-bitmap", "");
    std::fs::create_dir_all(&directory).unwrap();
    let snapshot = directory.join("data.snap");
    let path = snapshot.to_str().unwrap();
    {
        let primary = common::Server::start_with(&["--snapshot", path]);
        let mut writer = Client::new(&primary);
        assert_eq!(
            writer.call(&[b"ZADD", b"ranks", b"1", b"a", b"2", b"b"]),
            integer(2)
        );
        assert_eq!(writer.call(&[b"SETBIT", b"flags", b"9", b"1"]), integer(0));
        assert_eq!(writer.call(&[b"EXPIRE", b"ranks", b"100"]), integer(1));
        let port = primary.port.to_string();
        let replica = common::Server::start_with(&["--replicaof", "127.0.0.1", &port]);
        let mut reader = Client::new(&replica);
        wait_for(&mut reader, &[b"ZSCORE", b"ranks", b"b"], bulk(b"2"));
        wait_for(&mut reader, &[b"GETBIT", b"flags", b"9"], integer(1));
        assert_eq!(reader.call(&[b"WATCH", b"ranks"]), ok());
        assert_eq!(writer.call(&[b"ZINCRBY", b"ranks", b"1", b"b"]), bulk(b"3"));
        wait_for(&mut reader, &[b"ZSCORE", b"ranks", b"b"], bulk(b"3"));
        assert_eq!(reader.call(&[b"MULTI"]), ok());
        assert_eq!(reader.call(&[b"EXEC"]), Reply::Array(None));
        assert!(matches!(
            reader.call(&[b"SETBIT", b"flags", b"0", b"1"]),
            Reply::Error(_)
        ));
    }
    {
        let server = common::Server::start_with(&["--snapshot", path]);
        let mut c = Client::new(&server);
        assert_eq!(
            c.call(&[b"ZRANGE", b"ranks", b"0", b"-1"]),
            array(vec![bulk(b"a"), bulk(b"b")])
        );
        assert_eq!(c.call(&[b"ZSCORE", b"ranks", b"b"]), bulk(b"3"));
        assert_eq!(c.call(&[b"GETBIT", b"flags", b"9"]), integer(1));
        let Reply::Integer(ttl) = c.call(&[b"TTL", b"ranks"]) else {
            panic!("expected TTL")
        };
        assert!((95..=100).contains(&ttl));
    }
    std::fs::remove_file(snapshot).unwrap();
    std::fs::remove_file(directory.join("data.bak")).unwrap();
    std::fs::remove_dir(directory).unwrap();
}
