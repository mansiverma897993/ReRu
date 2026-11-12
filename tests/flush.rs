// SPDX-License-Identifier: AGPL-3.0-only
#[path = "common/client.rs"]
#[allow(dead_code)]
mod client;
#[allow(dead_code)]
mod common;

use client::{array, integer, ok, queued, Client, Reply};
use common::Server;

#[test]
fn flush_clears_all_types_and_invalidates_watches() {
    let server = Server::start();
    let mut watched = Client::new(&server);
    let mut writer = Client::new(&server);
    assert_eq!(writer.call(&[b"SET", b"string", b"value"]), ok());
    assert_eq!(writer.call(&[b"RPUSH", b"list", b"item"]), integer(1));
    assert_eq!(
        writer.call(&[b"GEOADD", b"geo", b"0", b"0", b"origin"]),
        integer(1)
    );
    assert_eq!(watched.call(&[b"WATCH", b"string", b"missing"]), ok());
    assert_eq!(writer.call(&[b"FLUSHDB", b"SYNC"]), ok());
    assert_eq!(writer.call(&[b"DBSIZE"]), integer(0));
    assert_eq!(watched.call(&[b"MULTI"]), ok());
    assert_eq!(watched.call(&[b"GET", b"string"]), queued());
    assert_eq!(watched.call(&[b"EXEC"]), Reply::Array(None));
    assert_eq!(writer.call(&[b"SET", b"after", b"value"]), ok());
    assert_eq!(writer.call(&[b"FLUSHALL", b"ASYNC"]), ok());
    assert_eq!(writer.call(&[b"DBSIZE"]), integer(0));
    assert!(matches!(
        writer.call(&[b"FLUSHDB", b"invalid"]),
        Reply::Error(_)
    ));
}

#[test]
fn flush_executes_in_transactions_and_survives_aof_restart() {
    let path = common::temp_path("reru-flush", ".aof");
    let filename = path.to_str().unwrap();
    {
        let server = Server::start_with(&["--appendonly", filename]);
        let mut c = Client::new(&server);
        assert_eq!(c.call(&[b"SET", b"old", b"value"]), ok());
        assert_eq!(c.call(&[b"MULTI"]), ok());
        assert_eq!(c.call(&[b"FLUSHDB"]), queued());
        assert_eq!(c.call(&[b"GET", b"old"]), queued());
        assert_eq!(c.call(&[b"EXEC"]), array(vec![ok(), Reply::Bulk(None)]));
    }
    {
        let server = Server::start_with(&["--appendonly", filename]);
        let mut c = Client::new(&server);
        assert_eq!(c.call(&[b"DBSIZE"]), integer(0));
        assert_eq!(c.call(&[b"GET", b"old"]), Reply::Bulk(None));
    }
    std::fs::remove_file(path).unwrap();
}
