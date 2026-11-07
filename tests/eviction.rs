// SPDX-License-Identifier: AGPL-3.0-only
#[path = "common/client.rs"]
#[allow(dead_code)]
mod client;
#[allow(dead_code)]
mod common;

use client::{array, ok, queued, Client, Reply};
use common::Server;

fn bulk(value: &[u8]) -> Reply {
    Reply::Bulk(Some(value.to_vec()))
}

#[test]
fn noeviction_rejects_growth_without_changing_data_or_watches() {
    let server = Server::start_with(&["--maxmemory", "220", "--maxmemory-policy", "noeviction"]);
    let mut client = Client::new(&server);
    let mut watcher = Client::new(&server);
    let value = vec![b'x'; 100];
    assert_eq!(client.call(&[b"SET", b"a", &value]), ok());
    assert_eq!(watcher.call(&[b"WATCH", b"b"]), ok());
    assert!(matches!(
        client.call(&[b"SET", b"b", &value]),
        Reply::Error(message) if message.starts_with(b"OOM ")
    ));
    assert_eq!(client.call(&[b"GET", b"a"]), bulk(&value));
    assert_eq!(client.call(&[b"GET", b"b"]), Reply::Bulk(None));
    assert_eq!(watcher.call(&[b"MULTI"]), ok());
    assert_eq!(watcher.call(&[b"GET", b"a"]), queued());
    assert_eq!(watcher.call(&[b"EXEC"]), array(vec![bulk(&value)]));
}

#[test]
fn allkeys_lru_evicts_oldest_accessed_key() {
    let server = Server::start_with(&["--maxmemory", "330", "--maxmemory-policy", "allkeys-lru"]);
    let mut client = Client::new(&server);
    let value = vec![b'x'; 100];
    assert_eq!(client.call(&[b"SET", b"a", &value]), ok());
    assert_eq!(client.call(&[b"SET", b"b", &value]), ok());
    assert_eq!(client.call(&[b"GET", b"a"]), bulk(&value));
    assert_eq!(client.call(&[b"SET", b"c", &value]), ok());
    assert_eq!(client.call(&[b"GET", b"b"]), Reply::Bulk(None));
    assert_eq!(client.call(&[b"GET", b"a"]), bulk(&value));
    assert_eq!(client.call(&[b"GET", b"c"]), bulk(&value));
}

#[test]
fn noeviction_is_atomic_for_multikey_and_transaction_writes() {
    let server = Server::start_with(&["--maxmemory", "240"]);
    let mut client = Client::new(&server);
    let value = vec![b'x'; 100];
    assert_eq!(client.call(&[b"SET", b"a", &value]), ok());
    assert!(matches!(
        client.call(&[b"MSET", b"b", &value, b"c", &value]),
        Reply::Error(message) if message.starts_with(b"OOM ")
    ));
    assert_eq!(client.call(&[b"GET", b"b"]), Reply::Bulk(None));
    assert_eq!(client.call(&[b"GET", b"c"]), Reply::Bulk(None));

    assert_eq!(client.call(&[b"MULTI"]), ok());
    assert_eq!(client.call(&[b"HSET", b"h", b"f", &value]), queued());
    assert_eq!(client.call(&[b"GET", b"a"]), queued());
    let reply = client.call(&[b"EXEC"]);
    assert!(matches!(
        reply,
        Reply::Array(Some(values)) if matches!(&values[0], Reply::Error(message) if message.starts_with(b"OOM ")) && values[1] == bulk(&value)
    ));
    assert_eq!(
        client.call(&[b"TYPE", b"h"]),
        Reply::Simple(b"none".to_vec())
    );
}

#[test]
fn lru_evictions_are_saved_in_snapshot() {
    let path = std::env::temp_dir().join(format!("reru-eviction-{}.snap", std::process::id()));
    let path_text = path.to_str().unwrap();
    let value = vec![b'v'; 100];
    {
        let server = Server::start_with(&[
            "--snapshot",
            path_text,
            "--maxmemory",
            "330",
            "--maxmemory-policy",
            "allkeys-lru",
        ]);
        let mut client = Client::new(&server);
        for key in [b"a", b"b", b"c"] {
            assert_eq!(client.call(&[b"SET", key, &value]), ok());
        }
        assert_eq!(client.call(&[b"GET", b"a"]), Reply::Bulk(None));
    }
    {
        let server = Server::start_with(&["--snapshot", path_text]);
        let mut client = Client::new(&server);
        assert_eq!(client.call(&[b"GET", b"a"]), Reply::Bulk(None));
        assert_eq!(client.call(&[b"GET", b"b"]), bulk(&value));
        assert_eq!(client.call(&[b"GET", b"c"]), bulk(&value));
    }
    {
        let server = Server::start_with(&[
            "--snapshot",
            path_text,
            "--maxmemory",
            "165",
            "--maxmemory-policy",
            "allkeys-lru",
        ]);
        let mut client = Client::new(&server);
        assert_eq!(client.call(&[b"GET", b"b"]), Reply::Bulk(None));
        assert_eq!(client.call(&[b"GET", b"c"]), bulk(&value));
    }
    std::fs::remove_file(path).unwrap();
}

#[test]
fn config_get_set_changes_memory_policy_at_runtime() {
    let server = Server::start();
    let mut client = Client::new(&server);
    let mut watcher = Client::new(&server);
    let value = vec![b'q'; 100];
    assert_eq!(
        client.call(&[b"CONFIG", b"GET", b"maxmemory", b"maxmemory-policy"]),
        array(vec![
            bulk(b"maxmemory"),
            bulk(b"0"),
            bulk(b"maxmemory-policy"),
            bulk(b"noeviction"),
        ])
    );
    assert_eq!(
        client.call(&[b"CONFIG", b"SET", b"maxmemory", b"330"]),
        ok()
    );
    assert_eq!(client.call(&[b"SET", b"a", &value]), ok());
    assert_eq!(client.call(&[b"SET", b"b", &value]), ok());
    assert_eq!(watcher.call(&[b"WATCH", b"a"]), ok());
    assert!(matches!(
        client.call(&[b"CONFIG", b"SET", b"maxmemory-policy", b"bogus"]),
        Reply::Error(_)
    ));
    assert_eq!(
        client.call(&[b"CONFIG", b"GET", b"*policy"]),
        array(vec![bulk(b"maxmemory-policy"), bulk(b"noeviction")])
    );
    assert_eq!(
        client.call(&[
            b"CONFIG",
            b"SET",
            b"maxmemory-policy",
            b"allkeys-lru",
            b"maxmemory",
            b"165"
        ]),
        ok()
    );
    assert_eq!(client.call(&[b"GET", b"a"]), Reply::Bulk(None));
    assert_eq!(client.call(&[b"GET", b"b"]), bulk(&value));
    assert_eq!(watcher.call(&[b"MULTI"]), ok());
    assert_eq!(watcher.call(&[b"GET", b"b"]), queued());
    assert_eq!(watcher.call(&[b"EXEC"]), Reply::Array(None));
}

#[test]
fn volatile_policies_only_evict_keys_with_ttl_and_roll_back_when_none_remain() {
    for policy in [
        "volatile-lru",
        "volatile-lrm",
        "volatile-lfu",
        "volatile-random",
        "volatile-ttl",
    ] {
        let server = Server::start_with(&["--maxmemory", "340", "--maxmemory-policy", policy]);
        let mut client = Client::new(&server);
        let value = vec![b'v'; 100];
        assert_eq!(
            client.call(&[b"SET", b"ttl", &value, b"PX", b"60000"]),
            ok()
        );
        assert_eq!(client.call(&[b"SET", b"keep", &value]), ok());
        assert_eq!(client.call(&[b"SET", b"next", &value]), ok());
        assert_eq!(client.call(&[b"GET", b"ttl"]), Reply::Bulk(None));
        assert_eq!(client.call(&[b"GET", b"keep"]), bulk(&value));
        assert_eq!(client.call(&[b"GET", b"next"]), bulk(&value));
        assert!(matches!(
            client.call(&[b"SET", b"last", &value]),
            Reply::Error(message) if message.starts_with(b"OOM ")
        ));
        assert_eq!(client.call(&[b"GET", b"last"]), Reply::Bulk(None));
        assert_eq!(client.call(&[b"DBSIZE"]), Reply::Integer(2));
    }
}

#[test]
fn volatile_ttl_evicts_shortest_remaining_deadline() {
    let server = Server::start_with(&["--maxmemory", "340", "--maxmemory-policy", "volatile-ttl"]);
    let mut client = Client::new(&server);
    let value = vec![b'v'; 100];
    assert_eq!(
        client.call(&[b"SET", b"fast", &value, b"PX", b"10000"]),
        ok()
    );
    assert_eq!(
        client.call(&[b"SET", b"slow", &value, b"PX", b"60000"]),
        ok()
    );
    assert_eq!(client.call(&[b"SET", b"keep", &value]), ok());
    assert_eq!(client.call(&[b"GET", b"fast"]), Reply::Bulk(None));
    assert_eq!(client.call(&[b"GET", b"slow"]), bulk(&value));
    assert_eq!(client.call(&[b"GET", b"keep"]), bulk(&value));
}

#[test]
fn allkeys_lfu_and_lrm_choose_the_cold_or_old_write() {
    let value = vec![b'v'; 100];
    {
        let server =
            Server::start_with(&["--maxmemory", "345", "--maxmemory-policy", "allkeys-lfu"]);
        let mut client = Client::new(&server);
        assert_eq!(client.call(&[b"SET", b"cold", &value]), ok());
        assert_eq!(client.call(&[b"SET", b"hot", &value]), ok());
        for _ in 0..8 {
            assert_eq!(client.call(&[b"GET", b"hot"]), bulk(&value));
        }
        assert_eq!(client.call(&[b"SET", b"new", &value]), ok());
        assert_eq!(client.call(&[b"GET", b"cold"]), Reply::Bulk(None));
        assert_eq!(client.call(&[b"GET", b"hot"]), bulk(&value));
    }
    {
        let server =
            Server::start_with(&["--maxmemory", "345", "--maxmemory-policy", "allkeys-lrm"]);
        let mut client = Client::new(&server);
        assert_eq!(client.call(&[b"SET", b"recent", &value]), ok());
        assert_eq!(client.call(&[b"SET", b"older", &value]), ok());
        assert_eq!(client.call(&[b"SET", b"recent", &value]), ok());
        assert_eq!(client.call(&[b"SET", b"new", &value]), ok());
        assert_eq!(client.call(&[b"GET", b"older"]), Reply::Bulk(None));
        assert_eq!(client.call(&[b"GET", b"recent"]), bulk(&value));
    }
}

#[test]
fn allkeys_random_enforces_limit_and_runtime_policy_name() {
    let server = Server::start();
    let mut client = Client::new(&server);
    assert_eq!(
        client.call(&[
            b"CONFIG",
            b"SET",
            b"maxmemory",
            b"330",
            b"maxmemory-policy",
            b"allkeys-random"
        ]),
        ok()
    );
    assert_eq!(
        client.call(&[b"CONFIG", b"GET", b"maxmemory-policy"]),
        array(vec![bulk(b"maxmemory-policy"), bulk(b"allkeys-random")])
    );
    let value = vec![b'v'; 100];
    for key in [b"a", b"b", b"c"] {
        assert_eq!(client.call(&[b"SET", key, &value]), ok());
    }
    assert_eq!(client.call(&[b"DBSIZE"]), Reply::Integer(2));
}

#[test]
fn volatile_oom_preserves_data_and_watches() {
    let server = Server::start_with(&["--maxmemory", "220", "--maxmemory-policy", "volatile-lru"]);
    let mut client = Client::new(&server);
    let mut watcher = Client::new(&server);
    let value = vec![b'x'; 100];
    assert_eq!(client.call(&[b"SET", b"keep", &value]), ok());
    assert_eq!(watcher.call(&[b"WATCH", b"new"]), ok());
    assert!(matches!(
        client.call(&[b"SET", b"new", &value]),
        Reply::Error(message) if message.starts_with(b"OOM ")
    ));
    assert_eq!(client.call(&[b"GET", b"keep"]), bulk(&value));
    assert_eq!(client.call(&[b"GET", b"new"]), Reply::Bulk(None));
    assert_eq!(watcher.call(&[b"MULTI"]), ok());
    assert_eq!(watcher.call(&[b"GET", b"keep"]), queued());
    assert_eq!(watcher.call(&[b"EXEC"]), array(vec![bulk(&value)]));
}

#[test]
fn info_reports_estimated_memory_and_evictions() {
    let server = Server::start_with(&["--maxmemory", "330", "--maxmemory-policy", "allkeys-lru"]);
    let mut client = Client::new(&server);
    let value = vec![b'v'; 100];
    for key in [b"a", b"b", b"c"] {
        assert_eq!(client.call(&[b"SET", key, &value]), ok());
    }
    let Reply::Bulk(Some(memory)) = client.call(&[b"INFO", b"memory"]) else {
        panic!("INFO memory should return a bulk string");
    };
    let memory = String::from_utf8(memory).unwrap();
    assert!(memory.contains("used_memory:330\r\n"));
    assert!(memory.contains("maxmemory:330\r\n"));
    assert!(memory.contains("maxmemory_policy:allkeys-lru\r\n"));
    let Reply::Bulk(Some(stats)) = client.call(&[b"INFO", b"stats"]) else {
        panic!("INFO stats should return a bulk string");
    };
    assert!(String::from_utf8(stats)
        .unwrap()
        .contains("evicted_keys:1\r\n"));
}

#[test]
fn deleted_key_does_not_keep_its_old_lfu_history() {
    let server = Server::start_with(&["--maxmemory", "340", "--maxmemory-policy", "allkeys-lfu"]);
    let mut client = Client::new(&server);
    let value = vec![b'v'; 100];
    assert_eq!(client.call(&[b"SET", b"cold", &value]), ok());
    assert_eq!(client.call(&[b"SET", b"hot", &value]), ok());
    for _ in 0..5 {
        assert_eq!(client.call(&[b"GET", b"cold"]), bulk(&value));
        assert_eq!(client.call(&[b"GET", b"hot"]), bulk(&value));
    }
    assert_eq!(client.call(&[b"DEL", b"hot"]), Reply::Integer(1));
    assert_eq!(client.call(&[b"SET", b"hot", &value]), ok());
    assert_eq!(client.call(&[b"SET", b"new", &value]), ok());
    assert_eq!(client.call(&[b"GET", b"cold"]), bulk(&value));
    assert_eq!(client.call(&[b"GET", b"hot"]), Reply::Bulk(None));
    assert_eq!(client.call(&[b"GET", b"new"]), bulk(&value));
}
