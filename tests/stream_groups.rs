// SPDX-License-Identifier: AGPL-3.0-only
#[path = "common/client.rs"]
#[allow(dead_code)]
mod client;
#[allow(dead_code)]
mod common;
use client::{array, integer, ok, queued, Client, Reply};
use common::Server;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

fn bulk(value: &[u8]) -> Reply {
    Reply::Bulk(Some(value.to_vec()))
}
fn entry(id: &[u8], value: &[u8]) -> Reply {
    array(vec![bulk(id), array(vec![bulk(b"f"), bulk(value)])])
}
fn delivered(items: Vec<Reply>) -> Reply {
    array(vec![array(vec![bulk(b"s"), array(items)])])
}
fn seed(c: &mut Client) {
    for n in 1..=3 {
        let id = format!("{n}-0");
        assert_eq!(
            c.call(&[b"XADD", b"s", id.as_bytes(), b"f", id.as_bytes()]),
            bulk(id.as_bytes())
        );
    }
    assert_eq!(c.call(&[b"XGROUP", b"CREATE", b"s", b"g", b"0"]), ok());
}
fn read(c: &mut Client, who: &[u8], id: &[u8], count: &[u8]) -> Reply {
    c.call(&[
        b"XREADGROUP",
        b"GROUP",
        b"g",
        who,
        b"COUNT",
        count,
        b"STREAMS",
        b"s",
        id,
    ])
}
fn summary(count: i64, first: &[u8], last: &[u8], consumers: &[(&[u8], &[u8])]) -> Reply {
    array(vec![
        integer(count),
        bulk(first),
        bulk(last),
        array(
            consumers
                .iter()
                .map(|(name, n)| array(vec![bulk(name), bulk(n)]))
                .collect(),
        ),
    ])
}
fn wait(mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while !predicate() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
}
fn path() -> std::path::PathBuf {
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!("reru-groups-{}-{n}", std::process::id()))
}

#[test]
fn lifecycle_binary_names_key_ttl_and_errors_are_atomic() {
    let server = Server::start();
    let mut c = Client::new(&server);
    assert!(matches!(
        c.call(&[b"XGROUP", b"CREATE", b"s", b"g", b"0"]),
        Reply::Error(_)
    ));
    assert_eq!(
        c.call(&[b"XGROUP", b"CREATE", b"s", b"g\0\xff", b"$", b"MKSTREAM"]),
        ok()
    );
    assert_eq!(c.call(&[b"XLEN", b"s"]), integer(0));
    assert_eq!(c.call(&[b"PEXPIRE", b"s", b"60000"]), integer(1));
    assert!(
        matches!(c.call(&[b"XGROUP", b"CREATE", b"s", b"g\0\xff", b"0"]), Reply::Error(e) if e.starts_with(b"BUSYGROUP"))
    );
    assert_eq!(
        c.call(&[b"XGROUP", b"CREATECONSUMER", b"s", b"g\0\xff", b"c\0"]),
        integer(1)
    );
    assert_eq!(
        c.call(&[b"XGROUP", b"CREATECONSUMER", b"s", b"g\0\xff", b"c\0"]),
        integer(0)
    );
    assert!(matches!(c.call(&[b"PTTL", b"s"]), Reply::Integer(n) if n>0));
    assert_eq!(
        c.call(&[b"XGROUP", b"DELCONSUMER", b"s", b"g\0\xff", b"c\0"]),
        integer(0)
    );
    assert_eq!(
        c.call(&[b"XGROUP", b"DESTROY", b"s", b"g\0\xff"]),
        integer(1)
    );
    assert_eq!(
        c.call(&[b"XGROUP", b"DESTROY", b"s", b"g\0\xff"]),
        integer(0)
    );
    assert!(matches!(
        c.call(&[
            b"XGROUP",
            b"CREATE",
            b"absent",
            b"g",
            b"invalid",
            b"MKSTREAM"
        ]),
        Reply::Error(_)
    ));
    assert_eq!(c.call(&[b"EXISTS", b"absent"]), integer(0));
    c.call(&[b"SET", b"string", b"v"]);
    assert!(
        matches!(c.call(&[b"XGROUP", b"CREATE", b"string", b"g", b"0"]), Reply::Error(e) if e.starts_with(b"WRONGTYPE"))
    );
}

#[test]
fn groups_distribute_once_pending_replays_and_acknowledgements_handle_deleted_payloads() {
    let server = Server::start();
    let mut c = Client::new(&server);
    seed(&mut c);
    assert_eq!(
        read(&mut c, b"alice", b">", b"1"),
        delivered(vec![entry(b"1-0", b"1-0")])
    );
    assert_eq!(
        read(&mut c, b"bob", b">", b"1"),
        delivered(vec![entry(b"2-0", b"2-0")])
    );
    assert_eq!(
        c.call(&[
            b"XREADGROUP",
            b"GROUP",
            b"g",
            b"alice",
            b"NOACK",
            b"STREAMS",
            b"s",
            b">"
        ]),
        delivered(vec![entry(b"3-0", b"3-0")])
    );
    assert_eq!(
        c.call(&[b"XPENDING", b"s", b"g"]),
        summary(2, b"1-0", b"2-0", &[(b"alice", b"1"), (b"bob", b"1")])
    );
    assert_eq!(
        read(&mut c, b"alice", b"0", b"10"),
        delivered(vec![entry(b"1-0", b"1-0")])
    );
    let Reply::Array(Some(rows)) = c.call(&[b"XPENDING", b"s", b"g", b"-", b"+", b"10", b"alice"])
    else {
        panic!()
    };
    let Reply::Array(Some(row)) = &rows[0] else {
        panic!()
    };
    assert_eq!(row[3], integer(2));
    c.call(&[b"XDEL", b"s", b"1-0"]);
    assert_eq!(
        read(&mut c, b"alice", b"0", b"10"),
        delivered(vec![array(vec![bulk(b"1-0"), Reply::Array(None)])])
    );
    assert_eq!(
        c.call(&[b"XACK", b"s", b"g", b"1-0", b"1-0", b"2-0"]),
        integer(2)
    );
    assert_eq!(
        c.call(&[b"XPENDING", b"s", b"g"]),
        array(vec![
            integer(0),
            Reply::Bulk(None),
            Reply::Bulk(None),
            Reply::Array(None)
        ])
    );
    assert_eq!(read(&mut c, b"alice", b">", b"1"), Reply::Array(None));
    assert_eq!(read(&mut c, b"alice", b"0", b"1"), delivered(vec![]));
}

#[test]
fn claiming_tracks_ownership_retry_counts_idle_and_autoclaim_tombstone_cleanup() {
    let server = Server::start();
    let mut c = Client::new(&server);
    seed(&mut c);
    read(&mut c, b"alice", b">", b"3");
    assert_eq!(
        c.call(&[b"XCLAIM", b"s", b"g", b"bob", b"60000", b"1-0"]),
        array(vec![])
    );
    assert_eq!(
        c.call(&[
            b"XCLAIM",
            b"s",
            b"g",
            b"bob",
            b"0",
            b"1-0",
            b"RETRYCOUNT",
            b"8",
            b"IDLE",
            b"5000",
            b"JUSTID"
        ]),
        array(vec![bulk(b"1-0")])
    );
    let Reply::Array(Some(rows)) =
        c.call(&[b"XPENDING", b"s", b"g", b"IDLE", b"4000", b"-", b"+", b"10"])
    else {
        panic!()
    };
    assert_eq!(rows.len(), 1);
    let Reply::Array(Some(row)) = &rows[0] else {
        panic!()
    };
    assert_eq!(row[1], bulk(b"bob"));
    assert_eq!(row[3], integer(8));
    c.call(&[b"XDEL", b"s", b"2-0"]);
    assert_eq!(
        c.call(&[
            b"XAUTOCLAIM",
            b"s",
            b"g",
            b"carol",
            b"0",
            b"0-0",
            b"COUNT",
            b"1",
            b"JUSTID"
        ]),
        array(vec![bulk(b"2-0"), array(vec![bulk(b"1-0")]), array(vec![])])
    );
    assert_eq!(
        c.call(&[
            b"XAUTOCLAIM",
            b"s",
            b"g",
            b"carol",
            b"0",
            b"2-0",
            b"COUNT",
            b"1"
        ]),
        array(vec![
            bulk(b"0-0"),
            array(vec![entry(b"3-0", b"3-0")]),
            array(vec![bulk(b"2-0")])
        ])
    );
    assert_eq!(
        c.call(&[b"XGROUP", b"DELCONSUMER", b"s", b"g", b"carol"]),
        integer(2)
    );
    assert_eq!(
        c.call(&[b"XCLAIM", b"s", b"g", b"bob", b"0", b"1-0", b"FORCE", b"JUSTID"]),
        array(vec![bulk(b"1-0")])
    );
    assert_eq!(
        c.call(&[b"XPENDING", b"s", b"g"]),
        summary(1, b"1-0", b"1-0", &[(b"bob", b"1")])
    );
}

#[test]
fn blocking_delivery_timeout_group_destruction_and_exec_are_supported() {
    let server = Server::start();
    let mut c = Client::new(&server);
    c.call(&[b"XGROUP", b"CREATE", b"s", b"g", b"0", b"MKSTREAM"]);
    let port = server.port;
    let worker = std::thread::spawn(move || {
        let mut c = Client::on_port(port);
        c.call(&[
            b"XREADGROUP",
            b"GROUP",
            b"g",
            b"alice",
            b"BLOCK",
            b"1000",
            b"STREAMS",
            b"s",
            b">",
        ])
    });
    wait(
        || matches!(c.call(&[b"XINFO",b"CONSUMERS",b"s",b"g"]),Reply::Array(Some(rows)) if !rows.is_empty()),
    );
    c.call(&[b"XADD", b"s", b"1-0", b"f", b"v"]);
    assert_eq!(worker.join().unwrap(), delivered(vec![entry(b"1-0", b"v")]));
    let started = Instant::now();
    assert_eq!(
        c.call(&[
            b"XREADGROUP",
            b"GROUP",
            b"g",
            b"alice",
            b"BLOCK",
            b"50",
            b"STREAMS",
            b"s",
            b">"
        ]),
        Reply::Array(None)
    );
    assert!(started.elapsed() >= Duration::from_millis(40));
    c.call(&[b"MULTI"]);
    assert_eq!(
        c.call(&[
            b"XREADGROUP",
            b"GROUP",
            b"g",
            b"alice",
            b"BLOCK",
            b"0",
            b"STREAMS",
            b"s",
            b">"
        ]),
        queued()
    );
    assert_eq!(c.call(&[b"EXEC"]), array(vec![Reply::Array(None)]));
    let port = server.port;
    let worker = std::thread::spawn(move || {
        let mut c = Client::on_port(port);
        c.call(&[
            b"XREADGROUP",
            b"GROUP",
            b"g",
            b"bob",
            b"BLOCK",
            b"0",
            b"STREAMS",
            b"s",
            b">",
        ])
    });
    wait(
        || matches!(c.call(&[b"XINFO",b"CONSUMERS",b"s",b"g"]),Reply::Array(Some(rows)) if rows.len()==2),
    );
    c.call(&[b"XGROUP", b"DESTROY", b"s", b"g"]);
    assert!(matches!(worker.join().unwrap(),Reply::Error(e) if e.starts_with(b"NOGROUP")));
}

#[test]
fn multistream_validation_memory_rollback_and_watches_cover_pending_metadata() {
    let server = Server::start();
    let mut c = Client::new(&server);
    let mut watcher = Client::new(&server);
    seed(&mut c);
    assert!(
        matches!(c.call(&[b"XREADGROUP",b"GROUP",b"g",b"alice",b"STREAMS",b"s",b"missing",b">",b">"]),Reply::Error(e) if e.starts_with(b"NOGROUP"))
    );
    assert_eq!(c.call(&[b"XINFO", b"CONSUMERS", b"s", b"g"]), array(vec![]));
    watcher.call(&[b"WATCH", b"s"]);
    c.call(&[b"CONFIG", b"SET", b"maxmemory", b"1"]);
    assert!(matches!(read(&mut c,b"alice",b">",b"1"),Reply::Error(e) if e.starts_with(b"OOM")));
    assert_eq!(c.call(&[b"XINFO", b"CONSUMERS", b"s", b"g"]), array(vec![]));
    watcher.call(&[b"MULTI"]);
    watcher.call(&[b"XLEN", b"s"]);
    assert_eq!(watcher.call(&[b"EXEC"]), array(vec![integer(3)]));
    c.call(&[b"CONFIG", b"SET", b"maxmemory", b"0"]);
    watcher.call(&[b"WATCH", b"s"]);
    read(&mut c, b"alice", b">", b"1");
    watcher.call(&[b"MULTI"]);
    watcher.call(&[b"XLEN", b"s"]);
    assert_eq!(watcher.call(&[b"EXEC"]), Reply::Array(None));
}

#[test]
fn snapshots_aof_and_rewrite_preserve_groups_deleted_pending_and_empty_streams() {
    for mode in ["--snapshot", "--appendonly"] {
        let path = path();
        let filename = path.to_str().unwrap();
        {
            let server = Server::start_with(&[mode, filename]);
            let mut c = Client::new(&server);
            seed(&mut c);
            read(&mut c, b"alice", b">", b"2");
            c.call(&[b"XDEL", b"s", b"1-0"]);
            c.call(&[
                b"XCLAIM",
                b"s",
                b"g",
                b"bob",
                b"0",
                b"2-0",
                b"RETRYCOUNT",
                b"9",
                b"JUSTID",
            ]);
            c.call(&[b"XGROUP", b"CREATE", b"empty", b"temp", b"0", b"MKSTREAM"]);
            c.call(&[b"XGROUP", b"DESTROY", b"empty", b"temp"]);
            if mode == "--appendonly" {
                c.call(&[b"BGREWRITEAOF"]);
                wait(
                    || matches!(c.call(&[b"INFO",b"persistence"]),Reply::Bulk(Some(bytes)) if String::from_utf8_lossy(&bytes).contains("aof_rewrites:1\r\n")),
                );
            }
        }
        {
            let server = Server::start_with(&[mode, filename]);
            let mut c = Client::new(&server);
            assert_eq!(
                c.call(&[b"XPENDING", b"s", b"g"]),
                summary(2, b"1-0", b"2-0", &[(b"alice", b"1"), (b"bob", b"1")])
            );
            assert_eq!(
                read(&mut c, b"alice", b"0", b"10"),
                delivered(vec![array(vec![bulk(b"1-0"), Reply::Array(None)])])
            );
            let Reply::Array(Some(rows)) = c.call(&[b"XPENDING", b"s", b"g", b"2-0", b"2-0", b"1"])
            else {
                panic!()
            };
            let Reply::Array(Some(row)) = &rows[0] else {
                panic!()
            };
            assert_eq!(row[3], integer(9));
            assert_eq!(
                read(&mut c, b"carol", b">", b"10"),
                delivered(vec![entry(b"3-0", b"3-0")])
            );
            assert_eq!(
                c.call(&[b"XADD", b"empty", b"0-1", b"f", b"v"]),
                bulk(b"0-1")
            );
            assert_eq!(c.call(&[b"XADD", b"s", b"3-*", b"f", b"v"]), bulk(b"3-1"));
        }
        std::fs::remove_file(&path).unwrap();
        if mode == "--snapshot" {
            std::fs::remove_file(path.with_extension("bak")).ok();
        }
    }
}

#[test]
fn replica_observes_group_and_pending_changes_and_rejects_delivery_writes() {
    let primary = Server::start();
    let mut c = Client::new(&primary);
    seed(&mut c);
    read(&mut c, b"alice", b">", b"2");
    let replica = Server::start_with(&["--replicaof", "127.0.0.1", &primary.port.to_string()]);
    let mut r = Client::new(&replica);
    wait(|| r.call(&[b"XPENDING", b"s", b"g"]) == summary(2, b"1-0", b"2-0", &[(b"alice", b"2")]));
    assert!(matches!(read(&mut r,b"bob",b">",b"1"),Reply::Error(e) if e.starts_with(b"READONLY")));
    c.call(&[b"XACK", b"s", b"g", b"1-0"]);
    wait(|| r.call(&[b"XPENDING", b"s", b"g"]) == summary(1, b"2-0", b"2-0", &[(b"alice", b"1")]));
}

fn field<'a>(reply: &'a Reply, name: &[u8]) -> &'a Reply {
    let Reply::Array(Some(fields)) = reply else {
        panic!("expected map: {reply:?}")
    };
    &fields
        .as_chunks::<2>()
        .0
        .iter()
        .find(|pair| pair[0] == bulk(name))
        .unwrap()[1]
}
fn rows(reply: &Reply) -> &[Reply] {
    let Reply::Array(Some(rows)) = reply else {
        panic!("expected array: {reply:?}")
    };
    rows
}

#[test]
fn stream_diagnostics_include_pending_owners_limits_and_deleted_metadata() {
    let server = Server::start();
    let mut c = Client::new(&server);
    seed(&mut c);
    read(&mut c, b"alice", b">", b"2");
    read(&mut c, b"bob", b">", b"1");
    c.call(&[b"XDEL", b"s", b"1-0"]);
    c.call(&[b"WATCH", b"s"]);
    let info = c.call(&[b"XINFO", b"STREAM", b"s"]);
    assert_eq!(field(&info, b"length"), &integer(2));
    assert_eq!(field(&info, b"entries-added"), &integer(3));
    assert_eq!(field(&info, b"max-deleted-entry-id"), &bulk(b"1-0"));
    assert_eq!(field(&info, b"first-entry"), &entry(b"2-0", b"2-0"));
    assert_eq!(field(&info, b"last-entry"), &entry(b"3-0", b"3-0"));
    let full = c.call(&[b"XINFO", b"STREAM", b"s", b"FULL", b"COUNT", b"1"]);
    assert_eq!(rows(field(&full, b"entries")).len(), 1);
    let group = &rows(field(&full, b"groups"))[0];
    assert_eq!(field(group, b"pel-count"), &integer(3));
    assert_eq!(rows(field(group, b"pending")).len(), 1);
    assert_eq!(rows(&rows(field(group, b"pending"))[0])[0], bulk(b"1-0"));
    let consumers = rows(field(group, b"consumers"));
    assert_eq!(consumers.len(), 2);
    assert_eq!(field(&consumers[0], b"name"), &bulk(b"alice"));
    assert_eq!(field(&consumers[0], b"pel-count"), &integer(2));
    assert_eq!(rows(field(&consumers[0], b"pending")).len(), 1);
    let full = c.call(&[b"XINFO", b"STREAM", b"s", b"FULL", b"COUNT", b"0"]);
    assert_eq!(rows(field(&full, b"entries")).len(), 2);
    assert_eq!(
        rows(field(&rows(field(&full, b"groups"))[0], b"pending")).len(),
        3
    );
    assert!(matches!(
        c.call(&[b"XINFO", b"STREAM", b"s", b"COUNT", b"1"]),
        Reply::Error(_)
    ));
    assert!(matches!(
        c.call(&[b"XINFO", b"STREAM", b"s", b"FULL", b"COUNT", b"-1"]),
        Reply::Error(_)
    ));
    c.call(&[b"MULTI"]);
    c.call(&[b"XLEN", b"s"]);
    assert_eq!(c.call(&[b"EXEC"]), array(vec![integer(2)]));
}

#[test]
fn reset_ids_and_trimmed_pending_entries_survive_persistence_and_rewrite() {
    for mode in ["--snapshot", "--appendonly"] {
        let path = path();
        let filename = path.to_str().unwrap();
        {
            let server = Server::start_with(&[mode, filename]);
            let mut c = Client::new(&server);
            seed(&mut c);
            read(&mut c, b"alice", b">", b"3");
            assert_eq!(c.call(&[b"XTRIM", b"s", b"MAXLEN", b"0"]), integer(3));
            assert_eq!(c.call(&[b"XSETID", b"s", b"0-0"]), ok());
            assert_eq!(c.call(&[b"XADD", b"s", b"0-1", b"f", b"new"]), bulk(b"0-1"));
            c.call(&[b"XADD", b"deleted", b"9-0", b"f", b"v"]);
            c.call(&[b"XDEL", b"deleted", b"9-0"]);
            assert!(matches!(
                c.call(&[b"XSETID", b"deleted", b"1-0"]),
                Reply::Error(_)
            ));
            assert!(matches!(
                c.call(&[b"XSETID", b"deleted", b"1-0", b"MAXDELETEDID", b"0-0"]),
                Reply::Error(_)
            ));
            assert_eq!(
                c.call(&[b"XSETID", b"deleted", b"9-0", b"MAXDELETEDID", b"0-0"]),
                ok()
            );
            if mode == "--appendonly" {
                c.call(&[b"BGREWRITEAOF"]);
                wait(
                    || matches!(c.call(&[b"INFO", b"persistence"]), Reply::Bulk(Some(bytes)) if String::from_utf8_lossy(&bytes).contains("aof_rewrites:1\r\n")),
                );
            }
        }
        {
            let server = Server::start_with(&[mode, filename]);
            let mut c = Client::new(&server);
            assert_eq!(
                c.call(&[b"XPENDING", b"s", b"g"]),
                summary(3, b"1-0", b"3-0", &[(b"alice", b"3")])
            );
            assert_eq!(
                c.call(&[b"XRANGE", b"s", b"-", b"+"]),
                array(vec![entry(b"0-1", b"new")])
            );
            let info = c.call(&[b"XINFO", b"STREAM", b"s"]);
            assert_eq!(field(&info, b"entries-added"), &integer(4));
            assert_eq!(field(&info, b"last-generated-id"), &bulk(b"0-1"));
            assert_eq!(field(&info, b"max-deleted-entry-id"), &bulk(b"0-0"));
            let deleted = c.call(&[b"XINFO", b"STREAM", b"deleted"]);
            assert_eq!(field(&deleted, b"max-deleted-entry-id"), &bulk(b"9-0"));
            assert_eq!(
                read(&mut c, b"alice", b"0", b"10"),
                delivered(vec![
                    array(vec![bulk(b"1-0"), Reply::Array(None)]),
                    array(vec![bulk(b"2-0"), Reply::Array(None)]),
                    array(vec![bulk(b"3-0"), Reply::Array(None)]),
                ])
            );
        }
        std::fs::remove_file(&path).unwrap();
        if mode == "--snapshot" {
            std::fs::remove_file(path.with_extension("bak")).ok();
        }
    }
}

#[test]
fn competing_blocked_consumers_deliver_each_entry_once() {
    let server = Server::start();
    let mut c = Client::new(&server);
    c.call(&[b"XGROUP", b"CREATE", b"s", b"g", b"0", b"MKSTREAM"]);
    let handles: Vec<_> = (0..4)
        .map(|n| {
            let port = server.port;
            std::thread::spawn(move || {
                let mut client = Client::on_port(port);
                client.call(&[
                    b"XREADGROUP",
                    b"GROUP",
                    b"g",
                    n.to_string().as_bytes(),
                    b"COUNT",
                    b"1",
                    b"BLOCK",
                    b"1500",
                    b"STREAMS",
                    b"s",
                    b">",
                ])
            })
        })
        .collect();
    wait(|| rows(&c.call(&[b"XINFO", b"CONSUMERS", b"s", b"g"])).len() == 4);
    for n in 1..=4 {
        c.call(&[b"XADD", b"s", format!("{n}-0").as_bytes(), b"f", b"v"]);
    }
    let mut ids = Vec::new();
    for handle in handles {
        let result = handle.join().unwrap();
        let stream = &rows(&result)[0];
        let entry = &rows(&rows(stream)[1])[0];
        let Reply::Bulk(Some(id)) = &rows(entry)[0] else {
            panic!()
        };
        ids.push(id.clone());
    }
    ids.sort();
    assert_eq!(
        ids,
        vec![
            b"1-0".to_vec(),
            b"2-0".to_vec(),
            b"3-0".to_vec(),
            b"4-0".to_vec()
        ]
    );
    assert_eq!(rows(&c.call(&[b"XPENDING", b"s", b"g"]))[0], integer(4));
}

#[test]
fn consumer_group_sequences_match_reference_redis() {
    let Ok(port) = std::env::var("RERU_REDIS_REFERENCE_PORT") else {
        return;
    };
    let server = Server::start();
    let mut ours = Client::new(&server);
    let mut redis = Client::on_port(port.parse().unwrap());
    let key = b"reru-diff:consumer-groups";
    ours.call(&[b"DEL", key]);
    redis.call(&[b"DEL", key]);
    let commands: Vec<Vec<&[u8]>> = vec![
        vec![b"XGROUP", b"CREATE", key, b"g", b"0", b"MKSTREAM"],
        vec![b"XINFO", b"GROUPS", key],
        vec![b"XADD", key, b"1-0", b"f", b"v"],
        vec![b"XADD", key, b"2-0", b"f", b"w"],
        vec![
            b"XREADGROUP",
            b"GROUP",
            b"g",
            b"alice",
            b"COUNT",
            b"1",
            b"STREAMS",
            key,
            b">",
        ],
        vec![b"XPENDING", key, b"g"],
        vec![b"XINFO", b"GROUPS", key],
        vec![
            b"XREADGROUP",
            b"GROUP",
            b"g",
            b"alice",
            b"NOACK",
            b"STREAMS",
            key,
            b">",
        ],
        vec![b"XCLAIM", key, b"g", b"bob", b"0", b"1-0", b"JUSTID"],
        vec![b"XPENDING", key, b"g"],
        vec![b"XREADGROUP", b"GROUP", b"g", b"bob", b"STREAMS", key, b"0"],
        vec![b"XDEL", key, b"1-0"],
        vec![b"XREADGROUP", b"GROUP", b"g", b"bob", b"STREAMS", key, b"0"],
        vec![b"XAUTOCLAIM", key, b"g", b"carol", b"0", b"0-0"],
        vec![b"XPENDING", key, b"g"],
        vec![b"XGROUP", b"SETID", key, b"g", b"0"],
        vec![b"XINFO", b"GROUPS", key],
        vec![b"XGROUP", b"CREATECONSUMER", key, b"g", b"explicit"],
        vec![b"XGROUP", b"CREATECONSUMER", key, b"g", b"explicit"],
        vec![b"XGROUP", b"DELCONSUMER", key, b"g", b"explicit"],
        vec![b"XACK", key, b"g", b"2-0"],
        vec![b"XGROUP", b"DESTROY", key, b"g"],
        vec![
            b"XADD",
            key,
            b"NOMKSTREAM",
            b"MAXLEN",
            b"=",
            b"1",
            b"3-0",
            b"f",
            b"v",
        ],
        vec![b"XRANGE", key, b"-", b"+"],
        vec![b"XADD", key, b"MINID", b"4", b"4", b"f", b"w"],
        vec![b"XRANGE", key, b"-", b"+"],
        vec![b"XTRIM", key, b"MAXLEN", b"0"],
        vec![b"XSETID", key, b"1-0"],
        vec![b"DEL", key],
        vec![b"XADD", key, b"NOMKSTREAM", b"*", b"f", b"v"],
    ];
    for command in commands {
        assert_eq!(ours.call(&command), redis.call(&command), "{command:?}");
    }
}
