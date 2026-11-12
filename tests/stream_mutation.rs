// SPDX-License-Identifier: AGPL-3.0-only
#[path = "common/client.rs"]
#[allow(dead_code)]
mod client;
#[allow(dead_code)]
mod common;

use client::{array, integer, Client, Reply};

fn bulk(bytes: &[u8]) -> Reply {
    Reply::Bulk(Some(bytes.to_vec()))
}

#[test]
fn append_options_trim_atomically_preserve_ttl_and_skip_missing_streams() {
    use client::{ok, queued};
    let server = common::Server::start();
    let mut c = Client::new(&server);
    let mut watcher = Client::new(&server);
    watcher.call(&[b"WATCH", b"absent"]);
    assert_eq!(
        c.call(&[
            b"XADD",
            b"absent",
            b"NOMKSTREAM",
            b"MAXLEN",
            b"0",
            b"*",
            b"f",
            b"v"
        ]),
        Reply::Bulk(None)
    );
    assert_eq!(c.call(&[b"EXISTS", b"absent"]), integer(0));
    watcher.call(&[b"MULTI"]);
    watcher.call(&[b"EXISTS", b"absent"]);
    assert_eq!(watcher.call(&[b"EXEC"]), array(vec![integer(0)]));
    assert_eq!(c.call(&[b"XADD", b"s", b"1", b"f", b"a"]), bulk(b"1-0"));
    c.call(&[b"PEXPIRE", b"s", b"60000"]);
    let ttl = c.call(&[b"PEXPIRETIME", b"s"]);
    assert_eq!(
        c.call(&[
            b"XADD",
            b"s",
            b"NOMKSTREAM",
            b"MAXLEN",
            b"=",
            b"1",
            b"2-0",
            b"f",
            b"b"
        ]),
        bulk(b"2-0")
    );
    assert_eq!(c.call(&[b"PEXPIRETIME", b"s"]), ttl);
    assert_eq!(
        c.call(&[b"XRANGE", b"s", b"-", b"+"]),
        array(vec![array(vec![
            bulk(b"2-0"),
            array(vec![bulk(b"f"), bulk(b"b")])
        ])])
    );
    let before = c.call(&[b"XRANGE", b"s", b"-", b"+"]);
    watcher.call(&[b"WATCH", b"s"]);
    for bad in [
        vec![
            b"XADD".as_slice(),
            b"s",
            b"MAXLEN",
            b"-1",
            b"3-0",
            b"f",
            b"v",
        ],
        vec![
            b"XADD".as_slice(),
            b"s",
            b"MAXLEN",
            b"0",
            b"1-0",
            b"f",
            b"v",
        ],
        vec![
            b"XADD".as_slice(),
            b"s",
            b"MINID",
            b"bad",
            b"3-0",
            b"f",
            b"v",
        ],
        vec![b"XADD".as_slice(), b"s", b"MAXLEN", b"0", b"3-0", b"f"],
    ] {
        assert!(matches!(c.call(&bad), Reply::Error(_)));
    }
    watcher.call(&[b"MULTI"]);
    watcher.call(&[b"XLEN", b"s"]);
    assert_eq!(watcher.call(&[b"EXEC"]), array(vec![integer(1)]));
    assert_eq!(c.call(&[b"XRANGE", b"s", b"-", b"+"]), before);
    assert_eq!(c.call(&[b"MULTI"]), ok());
    assert_eq!(
        c.call(&[b"XADD", b"s", b"MINID", b"=", b"3-0", b"3-0", b"f", b"c"]),
        queued()
    );
    assert_eq!(c.call(&[b"EXEC"]), array(vec![bulk(b"3-0")]));
    assert_eq!(c.call(&[b"XLEN", b"s"]), integer(1));
    c.call(&[b"XADD", b"s", b"MAXLEN", b"0", b"4-0", b"f", b"d"]);
    assert_eq!(c.call(&[b"XLEN", b"s"]), integer(0));
    assert_eq!(
        c.call(&[b"XADD", b"s", b"NOMKSTREAM", b"4-*", b"f", b"e"]),
        bulk(b"4-1")
    );
    c.call(&[b"SET", b"wrong", b"v"]);
    assert!(
        matches!(c.call(&[b"XADD", b"wrong", b"NOMKSTREAM", b"*", b"f", b"v"]), Reply::Error(e) if e.starts_with(b"WRONGTYPE"))
    );
}

#[test]
fn stream_deletion_keeps_last_generated_id_and_empty_key() {
    let server = common::Server::start();
    let mut c = Client::new(&server);
    assert_eq!(c.call(&[b"XADD", b"s", b"1-0", b"f", b"a"]), bulk(b"1-0"));
    assert_eq!(c.call(&[b"XADD", b"s", b"2-0", b"f", b"b"]), bulk(b"2-0"));
    assert_eq!(c.call(&[b"XADD", b"s", b"3-0", b"f", b"c"]), bulk(b"3-0"));
    assert_eq!(c.call(&[b"XDEL", b"s", b"3-0", b"3-0", b"9-0"]), integer(1));
    assert_eq!(c.call(&[b"XTRIM", b"s", b"MAXLEN", b"0"]), integer(2));
    assert_eq!(c.call(&[b"XLEN", b"s"]), integer(0));
    assert_eq!(c.call(&[b"TYPE", b"s"]), Reply::Simple(b"stream".to_vec()));
    assert!(matches!(
        c.call(&[b"XADD", b"s", b"2-1", b"f", b"old"]),
        Reply::Error(_)
    ));
    assert_eq!(c.call(&[b"XADD", b"s", b"3-*", b"f", b"new"]), bulk(b"3-1"));
    assert_eq!(c.call(&[b"XTRIM", b"s", b"MINID", b"4-0"]), integer(1));
    assert_eq!(c.call(&[b"XREVRANGE", b"s", b"+", b"-"]), array(vec![]));
    assert!(matches!(c.call(&[b"XDEL", b"s", b"bad"]), Reply::Error(_)));
}

#[test]
fn stream_last_id_survives_snapshot_and_aof_restarts() {
    for (mode, extension) in [("--snapshot", "snap"), ("--appendonly", "aof")] {
        let path = common::temp_path(&format!("reru-stream-{mode}"), &format!(".{extension}"));
        let filename = path.to_str().unwrap();
        {
            let server = common::Server::start_with(&[mode, filename]);
            let mut c = Client::new(&server);
            assert_eq!(c.call(&[b"XADD", b"s", b"1-0", b"f", b"a"]), bulk(b"1-0"));
            assert_eq!(c.call(&[b"XADD", b"s", b"2-0", b"f", b"b"]), bulk(b"2-0"));
            assert_eq!(c.call(&[b"XDEL", b"s", b"2-0"]), integer(1));
            assert_eq!(c.call(&[b"XTRIM", b"s", b"MAXLEN", b"0"]), integer(1));
        }
        {
            let server = common::Server::start_with(&[mode, filename]);
            let mut c = Client::new(&server);
            assert_eq!(c.call(&[b"TYPE", b"s"]), Reply::Simple(b"stream".to_vec()));
            assert_eq!(c.call(&[b"XLEN", b"s"]), integer(0));
            assert_eq!(c.call(&[b"XADD", b"s", b"2-*", b"f", b"c"]), bulk(b"2-1"));
        }
        std::fs::remove_file(&path).unwrap();
        if mode == "--snapshot" {
            std::fs::remove_file(path.with_extension("bak")).ok();
        }
    }
}
