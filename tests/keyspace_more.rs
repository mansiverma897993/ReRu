// SPDX-License-Identifier: AGPL-3.0-only
#[path = "common/client.rs"]
#[allow(dead_code)]
mod client;
#[allow(dead_code)]
mod common;

use client::{integer, ok, Client, Reply};

fn bulk(value: &[u8]) -> Reply {
    Reply::Bulk(Some(value.to_vec()))
}

fn set_fields(client: &mut Client, key: &[u8], start: usize, end: usize, value: &[u8]) -> i64 {
    let mut args = vec![b"HSET".to_vec(), key.to_vec()];
    for index in start..end {
        args.push(format!("field-{index}").into_bytes());
        args.push(value.to_vec());
    }
    match client.call_many(&args) {
        Reply::Integer(count) => count,
        other => panic!("HSET failed: {other:?}"),
    }
}

fn memory_usage(client: &mut Client, key: &[u8]) -> i64 {
    match client.call(&[b"MEMORY", b"USAGE", key]) {
        Reply::Integer(bytes) => bytes,
        other => panic!("MEMORY USAGE failed: {other:?}"),
    }
}

fn used_memory(client: &mut Client) -> i64 {
    let Reply::Bulk(Some(info)) = client.call(&[b"INFO", b"memory"]) else {
        panic!("INFO memory should return a bulk string");
    };
    String::from_utf8(info)
        .unwrap()
        .lines()
        .find_map(|line| line.strip_prefix("used_memory:")?.parse().ok())
        .expect("INFO memory should report used_memory")
}

#[test]
fn rename_copy_and_renamenx_preserve_source_expiry() {
    let server = common::Server::start();
    let mut client = Client::new(&server);
    assert_eq!(
        client.call(&[b"SET", b"source", b"data", b"PX", b"6000"]),
        ok()
    );
    assert_eq!(client.call(&[b"COPY", b"source", b"copy"]), integer(1));
    assert!(matches!(client.call(&[b"PTTL", b"copy"]), Reply::Integer(value) if value > 0));
    assert_eq!(client.call(&[b"SET", b"target", b"old"]), ok());
    assert_eq!(client.call(&[b"RENAME", b"source", b"target"]), ok());
    assert_eq!(client.call(&[b"GET", b"source"]), Reply::Bulk(None));
    assert_eq!(client.call(&[b"GET", b"target"]), bulk(b"data"));
    assert!(matches!(client.call(&[b"PTTL", b"target"]), Reply::Integer(value) if value > 0));
    assert_eq!(client.call(&[b"COPY", b"target", b"copy"]), integer(0));
    assert_eq!(
        client.call(&[b"COPY", b"target", b"copy", b"REPLACE"]),
        integer(1)
    );
    assert_eq!(client.call(&[b"RENAMENX", b"target", b"copy"]), integer(0));
    assert_eq!(client.call(&[b"RENAMENX", b"target", b"moved"]), integer(1));
    assert_eq!(client.call(&[b"RENAME", b"moved", b"moved"]), ok());
    assert_eq!(client.call(&[b"RENAMENX", b"moved", b"moved"]), integer(0));
    assert!(matches!(
        client.call(&[b"RENAME", b"absent", b"x"]),
        Reply::Error(_)
    ));
}

#[test]
fn unlink_touch_and_dbsize_work_across_data_types() {
    let server = common::Server::start();
    let mut client = Client::new(&server);
    assert_eq!(client.call(&[b"SET", b"string", b"v"]), ok());
    assert_eq!(client.call(&[b"SADD", b"set", b"member"]), integer(1));
    assert_eq!(client.call(&[b"DBSIZE"]), integer(2));
    assert_eq!(
        client.call(&[b"TOUCH", b"string", b"set", b"absent"]),
        integer(2)
    );
    assert_eq!(
        client.call(&[b"UNLINK", b"string", b"set", b"absent"]),
        integer(2)
    );
    assert_eq!(client.call(&[b"DBSIZE"]), integer(0));
    assert_eq!(client.call(&[b"TOUCH", b"string"]), integer(0));
}

#[test]
fn copy_oom_rolls_back_and_preserves_watch() {
    let server = common::Server::start_with(&["--maxmemory", "220"]);
    let mut client = Client::new(&server);
    let mut watcher = Client::new(&server);
    let value = vec![b'a'; 100];
    assert_eq!(client.call(&[b"SET", b"a", &value]), ok());
    assert_eq!(watcher.call(&[b"WATCH", b"b"]), ok());
    assert!(
        matches!(client.call(&[b"COPY", b"a", b"b"]), Reply::Error(message) if message.starts_with(b"OOM "))
    );
    assert_eq!(client.call(&[b"GET", b"b"]), Reply::Bulk(None));
    assert_eq!(watcher.call(&[b"MULTI"]), ok());
    assert_eq!(watcher.call(&[b"GET", b"a"]), client::queued());
    assert_eq!(watcher.call(&[b"EXEC"]), client::array(vec![bulk(&value)]));
}

#[test]
fn touch_changes_lru_victim() {
    let server =
        common::Server::start_with(&["--maxmemory", "330", "--maxmemory-policy", "allkeys-lru"]);
    let mut client = Client::new(&server);
    let value = vec![b'v'; 100];
    assert_eq!(client.call(&[b"SET", b"a", &value]), ok());
    assert_eq!(client.call(&[b"SET", b"b", &value]), ok());
    assert_eq!(client.call(&[b"TOUCH", b"a"]), integer(1));
    assert_eq!(client.call(&[b"SET", b"c", &value]), ok());
    assert_eq!(client.call(&[b"GET", b"b"]), Reply::Bulk(None));
    assert_eq!(client.call(&[b"GET", b"a"]), bulk(&value));
}

#[test]
fn rename_to_a_blocked_list_key_wakes_the_waiter() {
    let server = common::Server::start();
    let mut client = Client::new(&server);
    let mut blocked = Client::new(&server);
    let waiter = std::thread::spawn(move || blocked.call(&[b"BLPOP", b"queue", b"2"]));
    std::thread::sleep(std::time::Duration::from_millis(80));
    assert_eq!(client.call(&[b"RPUSH", b"temporary", b"item"]), integer(1));
    assert_eq!(client.call(&[b"RENAME", b"temporary", b"queue"]), ok());
    assert_eq!(
        waiter.join().unwrap(),
        client::array(vec![bulk(b"queue"), bulk(b"item")])
    );
}

#[test]
fn brpop_reads_the_right_end_and_wakes_for_a_new_list() {
    let server = common::Server::start();
    let mut writer = Client::new(&server);
    assert_eq!(
        writer.call(&[b"RPUSH", b"existing", b"left", b"right"]),
        integer(2)
    );
    assert_eq!(
        writer.call(&[b"BRPOP", b"missing", b"existing", b"1"]),
        client::array(vec![bulk(b"existing"), bulk(b"right")])
    );
    assert_eq!(
        writer.call(&[b"LRANGE", b"existing", b"0", b"-1"]),
        client::array(vec![bulk(b"left")])
    );
    let mut blocked = Client::new(&server);
    let waiter = std::thread::spawn(move || blocked.call(&[b"BRPOP", b"new", b"2"]));
    std::thread::sleep(std::time::Duration::from_millis(80));
    assert_eq!(writer.call(&[b"LPUSH", b"new", b"a", b"b"]), integer(2));
    assert_eq!(
        waiter.join().unwrap(),
        client::array(vec![bulk(b"new"), bulk(b"a")])
    );
    assert_eq!(
        writer.call(&[b"LRANGE", b"new", b"0", b"-1"]),
        client::array(vec![bulk(b"b")])
    );
}

#[test]
fn object_encoding_reports_string_encodings() {
    let server = common::Server::start();
    let mut client = Client::new(&server);
    for (key, value, encoding) in [
        (
            b"integer".as_slice(),
            b"12345".as_slice(),
            b"int".as_slice(),
        ),
        (b"zero", b"0", b"int"),
        (b"negative", b"-42", b"int"),
        (b"leading-zero", b"0123", b"embstr"),
        (b"plus", b"+42", b"embstr"),
        (b"negative-zero", b"-0", b"embstr"),
        (b"empty", b"", b"embstr"),
        (b"overflow", b"123456789012345678901", b"embstr"),
        (b"short", &[b'x'; 44], b"embstr"),
        (b"long", &[b'x'; 45], b"raw"),
    ] {
        assert_eq!(client.call(&[b"SET", key, value]), ok());
        assert_eq!(client.call(&[b"OBJECT", b"ENCODING", key]), bulk(encoding));
    }
    assert_eq!(
        client.call(&[b"OBJECT", b"ENCODING", b"missing"]),
        Reply::Bulk(None)
    );
    assert_eq!(client.call(&[b"set", b"lower", b"123"]), ok());
    assert_eq!(
        client.call(&[b"object", b"encoding", b"lower"]),
        bulk(b"int")
    );
    let binary_key = b"binary\0key\xff";
    let binary_value = b"\xff\x00value";
    assert_eq!(client.call(&[b"SET", binary_key, binary_value]), ok());
    assert_eq!(
        client.call(&[b"OBJECT", b"ENCODING", binary_key]),
        bulk(b"embstr")
    );
}

#[test]
fn object_encoding_reports_collection_encodings() {
    let server = common::Server::start();
    let mut client = Client::new(&server);

    assert_eq!(
        client.call(&[b"RPUSH", b"list", b"a", b"b", b"c"]),
        integer(3)
    );
    assert_eq!(
        client.call(&[b"OBJECT", b"ENCODING", b"list"]),
        bulk(b"listpack")
    );
    let huge = vec![b'y'; 9000];
    assert_eq!(client.call(&[b"RPUSH", b"quicklist", &huge]), integer(1));
    assert_eq!(
        client.call(&[b"OBJECT", b"ENCODING", b"quicklist"]),
        bulk(b"quicklist")
    );
    let boundary = vec![b'z'; 8190];
    assert_eq!(
        client.call(&[b"RPUSH", b"boundary-list", &boundary]),
        integer(1)
    );
    assert_eq!(
        client.call(&[b"OBJECT", b"ENCODING", b"boundary-list"]),
        bulk(b"quicklist")
    );
    for chunk in 1..=3 {
        let mut queue = vec![b"RPUSH".to_vec(), b"many-small".to_vec()];
        queue.extend((0..1000).map(|_| b"x".to_vec()));
        assert_eq!(client.call_many(&queue), integer(chunk * 1000));
    }
    assert_eq!(
        client.call(&[b"OBJECT", b"ENCODING", b"many-small"]),
        bulk(b"quicklist")
    );

    assert_eq!(
        client.call(&[b"HSET", b"hash", b"field", b"value"]),
        integer(1)
    );
    assert_eq!(
        client.call(&[b"OBJECT", b"ENCODING", b"hash"]),
        bulk(b"listpack")
    );
    let long_value = vec![b'v'; 65];
    assert_eq!(
        client.call(&[b"HSET", b"bighash", b"field", &long_value]),
        integer(1)
    );
    assert_eq!(
        client.call(&[b"OBJECT", b"ENCODING", b"bighash"]),
        bulk(b"hashtable")
    );
    let exact_value = vec![b'v'; 64];
    assert_eq!(
        set_fields(&mut client, b"exacthash", 0, 256, &exact_value),
        256
    );
    assert_eq!(
        set_fields(&mut client, b"exacthash", 256, 512, &exact_value),
        256
    );
    assert_eq!(
        set_fields(&mut client, b"widehash", 0, 256, &exact_value),
        256
    );
    assert_eq!(
        set_fields(&mut client, b"widehash", 256, 512, &exact_value),
        256
    );
    assert_eq!(
        set_fields(&mut client, b"widehash", 512, 513, &exact_value),
        1
    );
    assert_eq!(
        client.call(&[b"OBJECT", b"ENCODING", b"exacthash"]),
        bulk(b"listpack")
    );
    assert_eq!(
        client.call(&[b"OBJECT", b"ENCODING", b"widehash"]),
        bulk(b"hashtable")
    );
    assert_eq!(
        client.call(&[b"HEXPIRE", b"widehash", b"100", b"FIELDS", b"1", b"field-0"]),
        client::array(vec![integer(1)])
    );
    assert_eq!(
        client.call(&[b"OBJECT", b"ENCODING", b"widehash"]),
        bulk(b"hashtable")
    );
    assert_eq!(
        client.call(&[b"HSET", b"ttl-hash", b"field", b"value"]),
        integer(1)
    );
    assert_eq!(
        client.call(&[b"HEXPIRE", b"ttl-hash", b"100", b"FIELDS", b"1", b"field"]),
        client::array(vec![integer(1)])
    );
    assert_eq!(
        client.call(&[b"OBJECT", b"ENCODING", b"ttl-hash"]),
        bulk(b"listpackex")
    );

    assert_eq!(
        client.call(&[b"SADD", b"intset", b"1", b"2", b"3"]),
        integer(3)
    );
    assert_eq!(
        client.call(&[b"OBJECT", b"ENCODING", b"intset"]),
        bulk(b"intset")
    );
    assert_eq!(
        client.call(&[b"SADD", b"set", b"a", b"b", b"c"]),
        integer(3)
    );
    assert_eq!(
        client.call(&[b"OBJECT", b"ENCODING", b"set"]),
        bulk(b"listpack")
    );
    let long_member = vec![b'm'; 65];
    assert_eq!(client.call(&[b"SADD", b"bigset", &long_member]), integer(1));
    assert_eq!(
        client.call(&[b"OBJECT", b"ENCODING", b"bigset"]),
        bulk(b"hashtable")
    );
    let mut intset = vec![b"SADD".to_vec(), b"wideset".to_vec()];
    intset.extend((1..=513).map(|value| value.to_string().into_bytes()));
    assert_eq!(client.call_many(&intset), integer(513));
    assert_eq!(
        client.call(&[b"OBJECT", b"ENCODING", b"wideset"]),
        bulk(b"hashtable")
    );
    let mut listpack_set = vec![b"SADD".to_vec(), b"listpack-set".to_vec()];
    listpack_set.extend((0..128).map(|index| format!("m{index}").into_bytes()));
    assert_eq!(client.call_many(&listpack_set), integer(128));
    assert_eq!(
        client.call(&[b"OBJECT", b"ENCODING", b"listpack-set"]),
        bulk(b"listpack")
    );
    assert_eq!(
        client.call(&[b"SADD", b"listpack-set", b"extra"]),
        integer(1)
    );
    assert_eq!(
        client.call(&[b"OBJECT", b"ENCODING", b"listpack-set"]),
        bulk(b"hashtable")
    );

    assert_eq!(
        client.call(&[b"ZADD", b"zset", b"1", b"a", b"2", b"b"]),
        integer(2)
    );
    assert_eq!(
        client.call(&[b"OBJECT", b"ENCODING", b"zset"]),
        bulk(b"listpack")
    );
    assert_eq!(
        client.call(&[b"ZADD", b"bigzset", b"1", &long_member]),
        integer(1)
    );
    assert_eq!(
        client.call(&[b"OBJECT", b"ENCODING", b"bigzset"]),
        bulk(b"skiplist")
    );
    let mut zset = vec![b"ZADD".to_vec(), b"widezset".to_vec()];
    for index in 0..129 {
        zset.push(index.to_string().into_bytes());
        zset.push(format!("member-{index}").into_bytes());
    }
    assert_eq!(client.call_many(&zset), integer(129));
    assert_eq!(
        client.call(&[b"OBJECT", b"ENCODING", b"widezset"]),
        bulk(b"skiplist")
    );
    let mut compact_zset = vec![b"ZADD".to_vec(), b"listpack-zset".to_vec()];
    for index in 0..128 {
        compact_zset.push(index.to_string().into_bytes());
        compact_zset.push(format!("member-{index}").into_bytes());
    }
    assert_eq!(client.call_many(&compact_zset), integer(128));
    assert_eq!(
        client.call(&[b"OBJECT", b"ENCODING", b"listpack-zset"]),
        bulk(b"listpack")
    );

    assert!(matches!(
        client.call(&[b"XADD", b"stream", b"*", b"field", b"value"]),
        Reply::Bulk(Some(_))
    ));
    assert_eq!(
        client.call(&[b"OBJECT", b"ENCODING", b"stream"]),
        bulk(b"stream")
    );
}

#[test]
fn object_refcount_is_one_and_missing_keys_reply_nil() {
    let server = common::Server::start();
    let mut client = Client::new(&server);
    assert_eq!(client.call(&[b"SET", b"key", b"value"]), ok());
    assert_eq!(client.call(&[b"OBJECT", b"REFCOUNT", b"key"]), integer(1));
    assert_eq!(
        client.call(&[b"OBJECT", b"REFCOUNT", b"missing"]),
        Reply::Bulk(None)
    );
    assert_eq!(
        client.call(&[b"SET", b"expiring", b"value", b"PX", b"10"]),
        ok()
    );
    std::thread::sleep(std::time::Duration::from_millis(40));
    assert_eq!(
        client.call(&[b"OBJECT", b"ENCODING", b"expiring"]),
        Reply::Bulk(None)
    );
}

#[test]
fn object_argument_errors_match_redis() {
    let server = common::Server::start();
    let mut client = Client::new(&server);
    assert_eq!(client.call(&[b"SET", b"key", b"value"]), ok());
    assert_eq!(
        client.call(&[b"OBJECT"]),
        Reply::Error(b"ERR wrong number of arguments for 'object' command".to_vec())
    );
    assert_eq!(
        client.call(&[b"OBJECT", b"ENCODING"]),
        Reply::Error(b"ERR wrong number of arguments for 'object|encoding' command".to_vec())
    );
    assert_eq!(
        client.call(&[b"OBJECT", b"REFCOUNT", b"key", b"extra"]),
        Reply::Error(b"ERR wrong number of arguments for 'object|refcount' command".to_vec())
    );
    assert_eq!(
        client.call(&[b"OBJECT", b"FOO", b"key"]),
        Reply::Error(b"ERR unknown subcommand 'FOO'. Try OBJECT HELP.".to_vec())
    );
}

#[test]
fn object_encoding_recomputes_after_a_value_shrinks() {
    // Redis keeps the larger encoding after a conversion. ReRu reports the
    // label for the current contents, as docs/COMPATIBILITY.md notes.
    let server = common::Server::start();
    let mut client = Client::new(&server);
    let value = b"value";
    assert_eq!(set_fields(&mut client, b"shrinking", 0, 256, value), 256);
    assert_eq!(set_fields(&mut client, b"shrinking", 256, 513, value), 257);
    assert_eq!(
        client.call(&[b"OBJECT", b"ENCODING", b"shrinking"]),
        bulk(b"hashtable")
    );
    let mut delete = vec![b"HDEL".to_vec(), b"shrinking".to_vec()];
    delete.extend((0..500).map(|index| format!("field-{index}").into_bytes()));
    assert_eq!(client.call_many(&delete), integer(500));
    assert_eq!(
        client.call(&[b"OBJECT", b"ENCODING", b"shrinking"]),
        bulk(b"listpack")
    );
}

#[test]
fn object_queue_errors_abort_the_transaction() {
    let server = common::Server::start();
    let mut client = Client::new(&server);
    assert_eq!(client.call(&[b"MULTI"]), ok());
    assert_eq!(
        client.call(&[b"OBJECT", b"ENCODING"]),
        Reply::Error(b"ERR wrong number of arguments for 'object|encoding' command".to_vec())
    );
    assert_eq!(
        client.call(&[b"EXEC"]),
        Reply::Error(b"EXECABORT Transaction discarded because of previous errors.".to_vec())
    );
    assert_eq!(client.call(&[b"MULTI"]), ok());
    assert_eq!(
        client.call(&[b"OBJECT", b"FOO", b"key"]),
        Reply::Error(b"ERR unknown subcommand 'FOO'. Try OBJECT HELP.".to_vec())
    );
    assert_eq!(
        client.call(&[b"EXEC"]),
        Reply::Error(b"EXECABORT Transaction discarded because of previous errors.".to_vec())
    );
    assert_eq!(client.call(&[b"MULTI"]), ok());
    assert_eq!(
        client.call(&[b"SET", b"queued", b"value"]),
        client::queued()
    );
    assert_eq!(
        client.call(&[b"OBJECT", b"ENCODING", b"queued"]),
        client::queued()
    );
    assert_eq!(
        client.call(&[b"EXEC"]),
        client::array(vec![ok(), bulk(b"embstr")])
    );
}

#[test]
fn object_encoding_does_not_change_lru_order() {
    let server =
        common::Server::start_with(&["--maxmemory", "500", "--maxmemory-policy", "allkeys-lru"]);
    let mut client = Client::new(&server);
    let value = vec![b'v'; 100];
    assert_eq!(client.call(&[b"SET", b"ENCODING", &value]), ok());
    assert_eq!(client.call(&[b"SET", b"k", &value]), ok());
    // OBJECT resolves its key without touching LRU, so ENCODING stays oldest.
    assert_eq!(client.call(&[b"OBJECT", b"ENCODING", b"k"]), bulk(b"raw"));
    assert_eq!(client.call(&[b"SET", b"c", &value]), ok());
    assert_eq!(client.call(&[b"EXISTS", b"ENCODING"]), integer(0));
    assert_eq!(client.call(&[b"GET", b"k"]), bulk(&value));
    assert_eq!(client.call(&[b"GET", b"c"]), bulk(&value));
}

#[test]
fn object_encoding_reports_current_contents_after_in_place_writes() {
    // Redis converts these values to `raw`; ReRu reports the content-based label.
    let server = common::Server::start();
    let mut client = Client::new(&server);
    assert_eq!(client.call(&[b"SET", b"bit", b"1"]), ok());
    assert_eq!(client.call(&[b"SETBIT", b"bit", b"0", b"1"]), integer(0));
    assert_eq!(
        client.call(&[b"OBJECT", b"ENCODING", b"bit"]),
        bulk(b"embstr")
    );
    assert_eq!(client.call(&[b"SET", b"appended", b"1"]), ok());
    assert_eq!(client.call(&[b"APPEND", b"appended", b"2"]), integer(2));
    assert_eq!(
        client.call(&[b"OBJECT", b"ENCODING", b"appended"]),
        bulk(b"int")
    );
    assert_eq!(client.call(&[b"SET", b"ranged", b"123"]), ok());
    assert_eq!(
        client.call(&[b"SETRANGE", b"ranged", b"0", b"x"]),
        integer(3)
    );
    assert_eq!(
        client.call(&[b"OBJECT", b"ENCODING", b"ranged"]),
        bulk(b"embstr")
    );
}

#[test]
fn memory_usage_grows_with_value_size_for_every_type() {
    let server = common::Server::start();
    let mut client = Client::new(&server);
    let mut total = 0;
    for (key, create, grow) in [
        (
            b"string".as_slice(),
            &[b"SET".as_slice(), b"string", b"value"] as &[&[u8]],
            &[b"APPEND".as_slice(), b"string", b"-and-more"] as &[&[u8]],
        ),
        (
            b"list",
            &[b"RPUSH", b"list", b"a"],
            &[b"RPUSH", b"list", b"b", b"c"],
        ),
        (
            b"hash",
            &[b"HSET", b"hash", b"field", b"value"],
            &[b"HSET", b"hash", b"other", b"value"],
        ),
        (b"set", &[b"SADD", b"set", b"a"], &[b"SADD", b"set", b"b"]),
        (
            b"zset",
            &[b"ZADD", b"zset", b"1", b"a"],
            &[b"ZADD", b"zset", b"2", b"b"],
        ),
    ] {
        assert!(!matches!(client.call(create), Reply::Error(_)));
        let small = memory_usage(&mut client, key);
        // The key name and the entry overhead are counted with the value.
        assert!(small > key.len() as i64);
        assert!(!matches!(client.call(grow), Reply::Error(_)));
        let large = memory_usage(&mut client, key);
        assert!(
            large > small,
            "{} did not grow: {small} then {large}",
            String::from_utf8_lossy(key)
        );
        // INFO memory sums the same per-key estimate over the whole dataset.
        total += large;
        assert_eq!(used_memory(&mut client), total);
    }
}

#[test]
fn memory_usage_replies_nil_for_missing_and_expired_keys() {
    let server = common::Server::start();
    let mut client = Client::new(&server);
    assert_eq!(
        client.call(&[b"MEMORY", b"USAGE", b"missing"]),
        Reply::Bulk(None)
    );
    assert_eq!(
        client.call(&[b"MEMORY", b"USAGE", b"missing", b"SAMPLES", b"5"]),
        Reply::Bulk(None)
    );
    assert_eq!(
        client.call(&[b"SET", b"expiring", b"value", b"PX", b"10"]),
        ok()
    );
    std::thread::sleep(std::time::Duration::from_millis(40));
    assert_eq!(
        client.call(&[b"MEMORY", b"USAGE", b"expiring"]),
        Reply::Bulk(None)
    );
}

#[test]
fn memory_usage_accepts_samples_and_leaves_the_key_unchanged() {
    let server = common::Server::start();
    let mut client = Client::new(&server);
    let mut watcher = Client::new(&server);
    assert_eq!(
        client.call(&[b"RPUSH", b"list", b"a", b"b", b"c"]),
        integer(3)
    );
    assert_eq!(
        client.call(&[b"SET", b"expiring", b"value", b"EX", b"100"]),
        ok()
    );
    assert_eq!(watcher.call(&[b"WATCH", b"list", b"expiring"]), ok());
    let expected = memory_usage(&mut client, b"list");
    // SAMPLES is validated and ignored, so every count reports the same bytes.
    for command in [
        &[b"MEMORY".as_slice(), b"USAGE", b"list", b"SAMPLES", b"0"] as &[&[u8]],
        &[b"MEMORY", b"USAGE", b"list", b"SAMPLES", b"5"],
        &[
            b"MEMORY",
            b"USAGE",
            b"list",
            b"SAMPLES",
            b"9223372036854775807",
        ],
        &[b"memory", b"usage", b"list"],
        &[b"Memory", b"Usage", b"list", b"samples", b"1"],
    ] {
        assert_eq!(client.call(command), integer(expected));
    }
    assert!(memory_usage(&mut client, b"expiring") > 0);

    // Reading the estimate is not a write: value, TTL and WATCH are untouched.
    assert_eq!(
        client.call(&[b"LRANGE", b"list", b"0", b"-1"]),
        client::array(vec![bulk(b"a"), bulk(b"b"), bulk(b"c")])
    );
    assert_eq!(client.call(&[b"TTL", b"list"]), integer(-1));
    assert_eq!(client.call(&[b"GET", b"expiring"]), bulk(b"value"));
    assert!(matches!(client.call(&[b"TTL", b"expiring"]), Reply::Integer(ttl) if ttl > 0));
    assert_eq!(watcher.call(&[b"MULTI"]), ok());
    assert_eq!(watcher.call(&[b"LLEN", b"list"]), client::queued());
    assert_eq!(watcher.call(&[b"EXEC"]), client::array(vec![integer(3)]));
}

#[test]
fn memory_usage_rejects_invalid_arguments() {
    let server = common::Server::start();
    let mut client = Client::new(&server);
    assert_eq!(client.call(&[b"SET", b"key", b"value"]), ok());
    assert_eq!(
        client.call(&[b"MEMORY"]),
        Reply::Error(b"ERR wrong number of arguments for 'memory' command".to_vec())
    );
    assert_eq!(
        client.call(&[b"MEMORY", b"USAGE"]),
        Reply::Error(b"ERR wrong number of arguments for 'memory|usage' command".to_vec())
    );
    assert_eq!(
        client.call(&[b"memory", b"usage"]),
        Reply::Error(b"ERR wrong number of arguments for 'memory|usage' command".to_vec())
    );
    assert_eq!(
        client.call(&[b"MEMORY", b"FOO", b"key"]),
        Reply::Error(b"ERR unknown subcommand 'FOO'. Try MEMORY HELP.".to_vec())
    );
    // Options are checked before the key is looked up, so a missing key
    // reports the same errors as an existing one.
    for key in [b"key".as_slice(), b"missing"] {
        for options in [
            &[b"SAMPLES".as_slice()] as &[&[u8]],
            &[b"SAMPLES", b"-1"],
            &[b"SAMPLES", b"5", b"extra"],
            &[b"COUNT", b"5"],
            &[b"extra"],
        ] {
            let mut command = vec![b"MEMORY".as_slice(), b"USAGE", key];
            command.extend_from_slice(options);
            assert_eq!(
                client.call(&command),
                Reply::Error(b"ERR syntax error".to_vec()),
                "{command:?}"
            );
        }
        for count in [b"abc".as_slice(), b"1.5", b"+5", b"9223372036854775808"] {
            assert_eq!(
                client.call(&[b"MEMORY", b"USAGE", key, b"SAMPLES", count]),
                Reply::Error(b"ERR value is not an integer or out of range".to_vec())
            );
        }
    }
    assert_eq!(client.call(&[b"GET", b"key"]), bulk(b"value"));
}

#[test]
fn memory_usage_queue_errors_abort_the_transaction() {
    let server = common::Server::start();
    let mut client = Client::new(&server);
    for (command, error) in [
        (
            &[b"MEMORY".as_slice()] as &[&[u8]],
            b"ERR wrong number of arguments for 'memory' command".as_slice(),
        ),
        (
            &[b"MEMORY", b"USAGE"],
            b"ERR wrong number of arguments for 'memory|usage' command",
        ),
        (
            &[b"MEMORY", b"FOO", b"key"],
            b"ERR unknown subcommand 'FOO'. Try MEMORY HELP.",
        ),
    ] {
        assert_eq!(client.call(&[b"MULTI"]), ok());
        assert_eq!(client.call(command), Reply::Error(error.to_vec()));
        assert_eq!(
            client.call(&[b"EXEC"]),
            Reply::Error(b"EXECABORT Transaction discarded because of previous errors.".to_vec())
        );
    }

    assert_eq!(client.call(&[b"MULTI"]), ok());
    assert_eq!(
        client.call(&[b"SET", b"queued", b"value"]),
        client::queued()
    );
    assert_eq!(
        client.call(&[b"MEMORY", b"USAGE", b"queued"]),
        client::queued()
    );
    assert_eq!(
        client.call(&[b"MEMORY", b"USAGE", b"queued", b"SAMPLES", b"5"]),
        client::queued()
    );
    assert_eq!(
        client.call(&[b"MEMORY", b"USAGE", b"missing"]),
        client::queued()
    );
    // MULTI checks the subcommand and its arity. Option syntax is reported by
    // EXEC as that command's reply and does not abort the transaction.
    assert_eq!(
        client.call(&[b"MEMORY", b"USAGE", b"queued", b"SAMPLES"]),
        client::queued()
    );
    let replies = client.call(&[b"EXEC"]);
    let expected = memory_usage(&mut client, b"queued");
    assert_eq!(
        replies,
        client::array(vec![
            ok(),
            integer(expected),
            integer(expected),
            Reply::Bulk(None),
            Reply::Error(b"ERR syntax error".to_vec()),
        ])
    );
}

#[test]
fn memory_usage_does_not_change_lru_order() {
    let value = vec![b'v'; 100];
    // The oldest key is evicted first. It must stay the oldest when MEMORY
    // measures it, and when it merely shares its name with the subcommand.
    for (oldest, measured) in [(b"a".as_slice(), b"a".as_slice()), (b"USAGE", b"b")] {
        let server = common::Server::start_with(&[
            "--maxmemory",
            "400",
            "--maxmemory-policy",
            "allkeys-lru",
        ]);
        let mut client = Client::new(&server);
        assert_eq!(client.call(&[b"SET", oldest, &value]), ok());
        assert_eq!(client.call(&[b"SET", b"b", &value]), ok());
        assert!(memory_usage(&mut client, measured) > 0);
        assert_eq!(client.call(&[b"SET", b"c", &value]), ok());
        assert_eq!(client.call(&[b"EXISTS", oldest]), integer(0));
        assert_eq!(client.call(&[b"GET", b"b"]), bulk(&value));
        assert_eq!(client.call(&[b"GET", b"c"]), bulk(&value));
    }
}

#[test]
fn copy_rejects_identical_source_and_destination() {
    let server = common::Server::start();
    let mut client = Client::new(&server);
    assert_eq!(client.call(&[b"SET", b"same", b"value"]), ok());
    for command in [
        &[b"COPY".as_slice(), b"same", b"same"] as &[&[u8]],
        &[b"COPY", b"same", b"same", b"REPLACE"],
    ] {
        assert_eq!(
            client.call(command),
            Reply::Error(b"ERR source and destination objects are the same".to_vec())
        );
    }
    assert_eq!(client.call(&[b"GET", b"same"]), bulk(b"value"));
    assert_eq!(client.call(&[b"RENAME", b"same", b"same"]), ok());

    // The same-object error precedes the existence check.
    assert_eq!(
        client.call(&[b"COPY", b"absent", b"absent"]),
        Reply::Error(b"ERR source and destination objects are the same".to_vec())
    );
    assert_eq!(
        client.call(&[b"SET", b"source", b"value", b"PX", b"90000"]),
        ok()
    );
    assert_eq!(client.call(&[b"SET", b"destination", b"old"]), ok());
    assert_eq!(
        client.call(&[b"COPY", b"source", b"destination"]),
        integer(0)
    );
    assert_eq!(
        client.call(&[b"COPY", b"source", b"destination", b"REPLACE"]),
        integer(1)
    );
    assert_eq!(
        client.call(&[b"COPY", b"source", b"destination", b"REPLACE", b"REPLACE"]),
        integer(1)
    );
    assert!(matches!(
        client.call(&[b"COPY", b"source", b"destination", b"REPLACE", b"extra"]),
        Reply::Error(message) if message == b"ERR syntax error"
    ));
    assert!(matches!(client.call(&[b"PTTL", b"destination"]), Reply::Integer(ttl) if ttl > 0));
}
