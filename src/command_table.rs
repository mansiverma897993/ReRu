// SPDX-License-Identifier: AGPL-3.0-only
use crate::protocol;

const COMMANDS: &[&str] = &[
    "acl|whoami",
    "append",
    "auth",
    "bgrewriteaof",
    "bgsave",
    "bitcount",
    "bitfield",
    "bitfield_ro",
    "bitop",
    "bitpos",
    "blmove",
    "blmpop",
    "blpop",
    "brpop",
    "brpoplpush",
    "bzmpop",
    "bzpopmax",
    "bzpopmin",
    "command",
    "command|count",
    "command|info",
    "client|getname",
    "client|id",
    "client|info",
    "client|setinfo",
    "client|setname",
    "config|get",
    "config|set",
    "copy",
    "decr",
    "decrby",
    "del",
    "discard",
    "dbsize",
    "dump",
    "echo",
    "exec",
    "exists",
    "expire",
    "expireat",
    "expiretime",
    "flushall",
    "flushdb",
    "geoadd",
    "geodist",
    "geohash",
    "geopos",
    "geosearch",
    "geosearchstore",
    "get",
    "getbit",
    "getdel",
    "getex",
    "getrange",
    "getset",
    "hello",
    "hdel",
    "hexists",
    "hget",
    "hgetall",
    "hincrby",
    "hincrbyfloat",
    "hkeys",
    "hlen",
    "hmget",
    "hmset",
    "hrandfield",
    "hscan",
    "hset",
    "hsetnx",
    "hstrlen",
    "hvals",
    "incr",
    "incrby",
    "incrbyfloat",
    "info",
    "keys",
    "lastsave",
    "lindex",
    "linsert",
    "llen",
    "lmove",
    "lpop",
    "lpos",
    "lpush",
    "lpushx",
    "lrange",
    "lrem",
    "lset",
    "ltrim",
    "lmpop",
    "mget",
    "mset",
    "msetnx",
    "multi",
    "pfadd",
    "pfcount",
    "pfmerge",
    "ping",
    "psubscribe",
    "psetex",
    "publish",
    "pubsub",
    "punsubscribe",
    "pexpire",
    "pexpireat",
    "pexpiretime",
    "pttl",
    "persist",
    "quit",
    "randomkey",
    "reset",
    "rename",
    "renamenx",
    "restore",
    "rpop",
    "rpoplpush",
    "rpush",
    "rpushx",
    "sadd",
    "save",
    "scan",
    "scard",
    "sdiff",
    "sdiffstore",
    "select",
    "set",
    "setbit",
    "setex",
    "setnx",
    "setrange",
    "sinter",
    "sintercard",
    "sinterstore",
    "sismember",
    "smembers",
    "smismember",
    "smove",
    "spop",
    "srandmember",
    "srem",
    "sscan",
    "strlen",
    "substr",
    "subscribe",
    "sunion",
    "sunionstore",
    "time",
    "touch",
    "ttl",
    "type",
    "unwatch",
    "unsubscribe",
    "unlink",
    "watch",
    "xack",
    "xadd",
    "xautoclaim",
    "xclaim",
    "xdel",
    "xgroup|create",
    "xgroup|createconsumer",
    "xgroup|delconsumer",
    "xgroup|destroy",
    "xgroup|setid",
    "xinfo|consumers",
    "xinfo|groups",
    "xinfo|stream",
    "xlen",
    "xread",
    "xreadgroup",
    "xpending",
    "xrange",
    "xrevrange",
    "xsetid",
    "xtrim",
    "zadd",
    "zcard",
    "zcount",
    "zdiff",
    "zdiffstore",
    "zincrby",
    "zinter",
    "zintercard",
    "zinterstore",
    "zlexcount",
    "zmscore",
    "zmpop",
    "zpopmax",
    "zpopmin",
    "zrange",
    "zrangebylex",
    "zrangebyscore",
    "zrangestore",
    "zrank",
    "zrem",
    "zremrangebylex",
    "zremrangebyrank",
    "zremrangebyscore",
    "zrevrange",
    "zrevrangebylex",
    "zrevrangebyscore",
    "zrevrank",
    "zscan",
    "zscore",
    "zunion",
    "zunionstore",
];

#[derive(Clone, Copy)]
struct Metadata {
    arity: i64,
    first_key: i64,
    last_key: i64,
    key_step: i64,
}

fn metadata(name: &str) -> Metadata {
    let arity = match name {
        "get" | "getdel" | "incr" | "decr" | "strlen" | "hgetall" | "hexists" | "hlen"
        | "hkeys" | "hvals" | "scard" | "smembers" | "type" | "ttl" | "pttl" | "persist"
        | "touch" | "dump" | "xlen" | "zcard" | "dbsize" | "randomkey" | "multi" | "exec"
        | "discard" | "unwatch" | "quit" | "client|id" | "client|getname" | "client|info" => 2,
        "echo" | "append" | "setnx" | "getset" | "decrby" | "incrby" | "incrbyfloat" | "hget"
        | "hstrlen" | "sismember" | "smove" | "zscore" | "zincrby" | "zrank" | "zrevrank"
        | "zcount" | "zlexcount" | "expire" | "pexpire" | "expireat" | "pexpireat"
        | "expiretime" | "pexpiretime" | "rename" | "renamenx" | "copy" | "restore" | "geodist"
        | "lindex" | "lrem" | "rpoplpush" => 3,
        "client|setname" => 3,
        "client|setinfo" | "config|set" => 4,
        "config|get" => -3,
        "acl|whoami" => 2,
        "xgroup|create" => -5,
        "xgroup|setid" => -4,
        "xgroup|destroy" => 4,
        "xgroup|createconsumer" | "xgroup|delconsumer" => 5,
        "xinfo|groups" | "xinfo|stream" => 3,
        "xinfo|consumers" => 4,
        "xack" => -4,
        "xpending" => -3,
        "xreadgroup" => -7,
        "xclaim" | "xautoclaim" => -6,
        "xsetid" => -3,
        "xadd" => -5,
        "xtrim" => -4,
        "xdel" => -3,
        "xrange" | "xrevrange" => -4,
        "xread" => -4,
        "select" => 2,
        "getbit" => 3,
        "hsetnx" | "hincrby" | "hincrbyfloat" => 4,
        "hset" => -4,
        "lmove" | "blmove" => 6,
        "set" => -3,
        "getex" | "lpop" | "rpop" => -2,
        "getrange" | "substr" | "lrange" | "ltrim" | "lset" | "linsert" => 4,
        "setex" | "psetex" | "setrange" => 4,
        "hmset" | "sadd" | "srem" | "zadd" | "zrem" | "pfadd" | "pfcount" | "pfmerge"
        | "geoadd" | "geopos" | "geohash" | "geosearch" | "zmscore" | "zrange" | "zrevrange"
        | "zrangebyscore" | "zrevrangebyscore" | "zrangebylex" | "zrevrangebylex"
        | "zremrangebyrank" | "zremrangebyscore" | "zremrangebylex" | "hdel" | "hmget"
        | "smismember" => -3,
        "mget" | "del" | "unlink" | "exists" | "watch" | "sunion" | "sinter" | "sdiff" => -2,
        "zunion" | "zinter" | "zdiff" => -3,
        "mset" | "msetnx" => -3,
        "rpush" | "lpush" | "rpushx" | "lpushx" | "blpop" | "brpop" | "bzpopmin" | "bzpopmax"
        | "subscribe" | "unsubscribe" | "psubscribe" | "punsubscribe" | "sintercard" | "hscan"
        | "sscan" | "zscan" | "hrandfield" | "spop" | "srandmember" => -3,
        "brpoplpush" => 4,
        "lmpop" | "zmpop" => -4,
        "blmpop" | "bzmpop" => -5,
        "bitop" => -4,
        "bitcount" | "bitpos" | "bitfield" | "bitfield_ro" => -2,
        "setbit" => 4,
        "geosearchstore" => -8,
        "zrangestore" => -5,
        "sunionstore" | "sinterstore" | "sdiffstore" => -3,
        "zunionstore" | "zinterstore" | "zdiffstore" => -4,
        "command" | "hello" | "ping" | "info" | "flushdb" | "flushall" | "time" | "bgsave"
        | "bgrewriteaof" | "pubsub" | "auth" => -1,
        "command|count" => 2,
        "command|info" => -2,
        "save" => 1,
        "reset" => 1,
        _ => -1,
    };
    let arity = match name {
        "echo" | "select" => 2,
        "dbsize" | "randomkey" | "multi" | "exec" | "discard" | "unwatch" | "quit" | "lastsave"
        | "save" | "time" => 1,
        "touch" | "hrandfield" | "spop" | "srandmember" | "zpopmin" | "zpopmax" | "pfadd"
        | "pfcount" | "geopos" | "geohash" => -2,
        "lrem" | "copy" => -3,
        "restore" => -4,
        "zadd" => -4,
        "zrank" | "zrevrank" | "lpos" => -3,
        "zremrangebyrank" | "zremrangebyscore" | "zremrangebylex" => 4,
        "geosearch" => -7,
        "geoadd" => -5,
        "geodist" => -4,
        "bitpos" => -3,
        "publish" => 4,
        "subscribe" | "psubscribe" => -2,
        "unsubscribe" | "punsubscribe" => -1,
        "scan" => -2,
        "keys" => 2,
        _ => arity,
    };

    let (first_key, last_key, key_step) = match name {
        "mget" | "del" | "unlink" | "exists" | "watch" | "sunion" | "sinter" | "sdiff" => {
            (1, -1, 1)
        }
        "mset" | "msetnx" => (1, -1, 2),
        "blmove" | "lmove" | "rpoplpush" | "brpoplpush" => (1, 2, 1),
        "blpop" | "brpop" | "bzpopmin" | "bzpopmax" => (1, -2, 1),
        "lmpop" => (2, -1, 1),
        "blmpop" => (3, -1, 1),
        "zmpop" => (2, -1, 1),
        "bzmpop" => (3, -1, 1),
        "bitop" => (2, -1, 1),
        "geosearchstore" => (1, 2, 1),
        "zrangestore" => (1, 1, 1),
        "sunionstore" | "sinterstore" | "sdiffstore" => (1, -1, 1),
        "xgroup|create"
        | "xgroup|setid"
        | "xgroup|destroy"
        | "xgroup|createconsumer"
        | "xgroup|delconsumer" => (2, 2, 1),
        "xinfo|groups" | "xinfo|consumers" | "xinfo|stream" => (2, 2, 1),
        "zunion" | "zinter" | "zdiff" => (2, -1, 1),
        "zunionstore" | "zinterstore" | "zdiffstore" => (1, -1, 1),
        "xread" | "xreadgroup" => (0, 0, 0),
        "command" | "command|count" | "command|info" | "ping" | "echo" | "hello" | "time"
        | "info" | "save" | "bgsave" | "lastsave" | "multi" | "exec" | "discard" | "unwatch"
        | "quit" | "auth" | "acl|whoami" | "client|id" | "client|getname" | "client|info"
        | "client|setname" | "client|setinfo" | "config|get" | "config|set" | "pubsub"
        | "subscribe" | "unsubscribe" | "psubscribe" | "punsubscribe" | "flushdb" | "flushall"
        | "select" | "reset" | "bgrewriteaof" | "scan" | "keys" => (0, 0, 0),
        _ => (1, 1, 1),
    };

    Metadata {
        arity,
        first_key,
        last_key,
        key_step,
    }
}

fn entry(name: &str) -> Vec<u8> {
    let info = metadata(name);
    let mut response = Vec::new();
    response.extend_from_slice(b"*6\r\n");
    response.extend_from_slice(&protocol::bulk(name.as_bytes()));
    response.extend_from_slice(format!(":{}\r\n", info.arity).as_bytes());
    response.extend_from_slice(b"*0\r\n");
    response.extend_from_slice(
        format!(
            ":{}\r\n:{}\r\n:{}\r\n",
            info.first_key, info.last_key, info.key_step
        )
        .as_bytes(),
    );
    response
}

fn nested_array(values: &[Vec<u8>]) -> Vec<u8> {
    let mut response = format!("*{}\r\n", values.len()).into_bytes();
    for value in values {
        response.extend_from_slice(value);
    }
    response
}

fn lookup(name: &[u8]) -> Option<&'static str> {
    COMMANDS
        .iter()
        .copied()
        .find(|candidate| candidate.as_bytes().eq_ignore_ascii_case(name))
}

pub fn supports(command: &[Vec<u8>]) -> bool {
    if command.is_empty() {
        return false;
    }
    if lookup(&command[0]).is_some() {
        return true;
    }
    command.get(1).is_some_and(|subcommand| {
        let candidate = format!(
            "{}|{}",
            String::from_utf8_lossy(&command[0]).to_ascii_lowercase(),
            String::from_utf8_lossy(subcommand).to_ascii_lowercase()
        );
        COMMANDS.contains(&candidate.as_str())
    })
}

pub fn valid_arity(command: &[Vec<u8>]) -> Option<bool> {
    if command
        .first()
        .is_none_or(|name| !name.eq_ignore_ascii_case(b"COMMAND"))
    {
        return None;
    }
    Some(
        match command
            .get(1)
            .map(|name| name.to_ascii_uppercase())
            .as_deref()
        {
            None => command.len() == 1,
            Some(b"COUNT") => command.len() == 2,
            Some(b"INFO") => true,
            _ => false,
        },
    )
}

pub fn execute(command: &[Vec<u8>]) -> Vec<u8> {
    if !valid_arity(command).unwrap_or(false) {
        return protocol::error("wrong number of arguments for 'command' command");
    }
    match command
        .get(1)
        .map(|name| name.to_ascii_uppercase())
        .as_deref()
    {
        Some(b"COUNT") => format!(":{}\r\n", COMMANDS.len()).into_bytes(),
        Some(b"INFO") => nested_array(
            &command[2..]
                .iter()
                .map(|name| {
                    lookup(name)
                        .map(entry)
                        .unwrap_or_else(|| b"$-1\r\n".to_vec())
                })
                .collect::<Vec<_>>(),
        ),
        _ => {
            let mut names = COMMANDS.to_vec();
            names.sort_unstable();
            nested_array(&names.iter().map(|name| entry(name)).collect::<Vec<_>>())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_metadata_entry_is_accepted_by_the_dispatcher_registry() {
        for name in COMMANDS {
            let parts: Vec<Vec<u8>> = name
                .split('|')
                .map(|part| part.as_bytes().to_vec())
                .collect();
            assert!(
                supports(&parts),
                "dispatcher command missing from table: {name}"
            );
            assert!(lookup(name.as_bytes()).is_some());
        }
        assert_eq!(COMMANDS.len(), 207);
    }
}
