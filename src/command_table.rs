// SPDX-License-Identifier: AGPL-3.0-only
use crate::protocol;
use std::collections::BTreeSet;

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
    "config|resetstat",
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
    "hexpire",
    "hexpireat",
    "hexpiretime",
    "hget",
    "hgetall",
    "hgetex",
    "hincrby",
    "hincrbyfloat",
    "hkeys",
    "hlen",
    "hmget",
    "hmset",
    "hpexpire",
    "hpexpireat",
    "hpexpiretime",
    "hpersist",
    "hpttl",
    "hrandfield",
    "hscan",
    "hset",
    "hsetex",
    "hsetnx",
    "hstrlen",
    "httl",
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Metadata {
    arity: i64,
    first_key: i64,
    last_key: i64,
    key_step: i64,
}

fn metadata(name: &str) -> Metadata {
    let arity = match name {
        "command" | "hello" | "ping" | "info" | "flushdb" | "flushall" | "bgsave"
        | "bgrewriteaof" | "quit" | "unsubscribe" | "punsubscribe" => -1,
        "command|count" | "client|getname" | "client|id" | "client|info" | "echo" | "select"
        | "get" | "getdel" | "incr" | "decr" | "strlen" | "hgetall" | "hlen" | "hkeys"
        | "hvals" | "scard" | "smembers" | "type" | "ttl" | "pttl" | "persist" | "dump"
        | "xlen" | "zcard" | "expiretime" | "pexpiretime" | "lastsave" | "dbsize" | "randomkey"
        | "multi" | "exec" | "discard" | "unwatch" | "save" | "time" | "reset" => 1,
        "client|setname" | "append" | "setnx" | "getset" | "decrby" | "incrby" | "incrbyfloat"
        | "hget" | "hstrlen" | "sismember" | "zscore" | "rpoplpush" | "rename" | "renamenx"
        | "lindex" | "hexists" => 3,
        "httl" | "hpttl" | "hexpiretime" | "hpexpiretime" | "hpersist" | "hgetex" => -5,
        "config|resetstat" => 2,
        "client|setinfo" | "getbit" | "hincrby" | "hincrbyfloat" | "hsetnx" | "smove" | "lset"
        | "lrem" | "zcount" | "zlexcount" | "zremrangebyrank" | "zremrangebyscore"
        | "zremrangebylex" => 4,
        "linsert" | "lmove" | "xgroup|createconsumer" | "xgroup|delconsumer" => 5,
        "blmove" => 6,
        "brpoplpush" => 4,
        "getrange" | "lrange" | "ltrim" | "substr" => 4,
        "restore" => -4,
        "zincrby" => 4,
        "client" | "config" | "acl" | "xgroup" | "xinfo" => -2,
        "config|get" => -3,
        "config|set" => -4,
        "acl|whoami" => 2,
        "auth" | "pubsub" | "getex" | "lpop" | "rpop" | "hrandfield" | "spop" | "srandmember"
        | "zpopmin" | "zpopmax" | "pfadd" | "pfcount" | "keys" | "scan" | "touch" | "mget"
        | "del" | "unlink" | "exists" | "watch" | "sunion" | "sinter" | "sdiff" | "bitcount"
        | "bitfield" | "bitfield_ro" | "command|info" => -2,
        "set" | "mset" | "msetnx" => -3,
        "setex" | "psetex" | "setrange" | "setbit" | "xgroup|destroy" => 4,
        "hset" | "hmset" | "zadd" => -4,
        "hexpire" | "hpexpire" | "hexpireat" | "hpexpireat" | "hsetex" => -6,
        "hdel" | "hmget" | "smismember" | "zmscore" | "xdel" | "hscan" | "sscan" | "zscan"
        | "sintercard" | "xpending" | "zrank" | "zrevrank" | "lpos" | "zunion" | "zinter"
        | "zdiff" | "pfmerge" | "geohash" | "expire" | "pexpire" | "expireat" | "pexpireat"
        | "copy" => -3,
        "sadd" | "srem" | "zrem" | "rpush" | "lpush" | "rpushx" | "lpushx" | "blpop" | "brpop"
        | "bzpopmin" | "bzpopmax" => -3,
        "subscribe" | "psubscribe" => -2,
        "lmpop" | "zmpop" => -4,
        "blmpop" | "bzmpop" => -5,
        "bitop" => -4,
        "bitpos" => -3,
        "xgroup|create" => -5,
        "xgroup|setid" => -4,
        "xinfo|groups" => 3,
        "xinfo|consumers" => 4,
        "xinfo|stream" => -3,
        "xack" => -4,
        "xreadgroup" => -7,
        "xclaim" | "xautoclaim" => -6,
        "xsetid" => -3,
        "xadd" => -5,
        "xtrim" => -4,
        "xrange" | "xrevrange" | "xread" | "zrange" | "zrevrange" | "zrangebyscore"
        | "zrevrangebyscore" | "zrangebylex" | "zrevrangebylex" => -4,
        "zrangestore" => -5,
        "zunionstore" | "zinterstore" | "zdiffstore" => -4,
        "sunionstore" | "sinterstore" | "sdiffstore" => -3,
        "geosearch" => -7,
        "geosearchstore" => -8,
        "geoadd" => -5,
        "geodist" => -4,
        "geopos" => -3,
        "publish" => 3,
        "zintercard" => -3,
        "llen" => 2,
        _ => panic!("missing command arity metadata: {name}"),
    };

    let (first_key, last_key, key_step) = match name {
        "command" | "command|count" | "command|info" | "acl" | "acl|whoami" | "auth"
        | "bgrewriteaof" | "bgsave" | "client" | "client|getname" | "client|id" | "client|info"
        | "client|setinfo" | "client|setname" | "config" | "config|get" | "config|resetstat"
        | "config|set" | "discard" | "dbsize" | "echo" | "exec" | "flushall" | "flushdb"
        | "hello" | "info" | "lastsave" | "multi" | "ping" | "pubsub" | "punsubscribe"
        | "psubscribe" | "publish" | "quit" | "randomkey" | "reset" | "save" | "scan"
        | "select" | "subscribe" | "time" | "unwatch" | "unsubscribe" | "xread" | "xreadgroup" => {
            (0, 0, 0)
        }
        "copy" | "rename" | "renamenx" | "smove" | "lmove" | "blmove" | "rpoplpush"
        | "brpoplpush" | "zrangestore" => (1, 2, 1),
        "geosearchstore" => (1, 2, 1),
        "sunionstore" | "sinterstore" | "sdiffstore" | "zunionstore" | "zinterstore"
        | "zdiffstore" => (1, -1, 1),
        "mset" | "msetnx" => (1, -1, 2),
        "mget" | "del" | "unlink" | "exists" | "watch" | "sunion" | "sinter" | "sdiff"
        | "pfcount" | "pfmerge" => (1, -1, 1),
        "zunion" | "zinter" | "zdiff" => (2, -1, 1),
        "blpop" | "brpop" | "bzpopmin" | "bzpopmax" => (1, -2, 1),
        "lmpop" | "zmpop" => (2, -1, 1),
        "blmpop" | "bzmpop" => (3, -1, 1),
        "bitop" => (2, -1, 1),
        "sintercard" => (2, -1, 1),
        "xgroup|create"
        | "xgroup|setid"
        | "xgroup|destroy"
        | "xgroup|createconsumer"
        | "xgroup|delconsumer"
        | "xinfo|groups"
        | "xinfo|consumers"
        | "xinfo|stream" => (2, 2, 1),
        _ => (1, 1, 1),
    };

    Metadata {
        arity,
        first_key,
        last_key,
        key_step,
    }
}

fn root_commands() -> Vec<String> {
    COMMANDS
        .iter()
        .map(|name| {
            name.split_once('|')
                .map_or(*name, |(root, _)| root)
                .to_owned()
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn children(command: &str) -> Vec<&'static str> {
    let mut children = COMMANDS
        .iter()
        .filter(|name| {
            name.split_once('|')
                .is_some_and(|(root, _)| root == command)
        })
        .copied()
        .collect::<Vec<_>>();
    children.sort_unstable();
    children
}

fn array(elements: &[Vec<u8>]) -> Vec<u8> {
    let mut response = format!("*{}\r\n", elements.len()).into_bytes();
    for element in elements {
        response.extend_from_slice(element);
    }
    response
}

fn entry(name: &str, include_subcommands: bool) -> Vec<u8> {
    let info = metadata(name);
    let subcommands = if include_subcommands {
        children(name)
            .iter()
            .map(|command| entry(command, false))
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    let mut response = Vec::new();
    response.extend_from_slice(b"*10\r\n");
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
    response.extend_from_slice(b"*0\r\n*0\r\n*0\r\n");
    response.extend_from_slice(&array(&subcommands));
    response
}

fn lookup(name: &[u8]) -> Option<String> {
    let name = String::from_utf8_lossy(name).to_ascii_lowercase();
    if COMMANDS.contains(&name.as_str()) {
        return Some(name);
    }
    root_commands()
        .into_iter()
        .find(|candidate| candidate == &name)
}

pub fn supports(command: &[Vec<u8>]) -> bool {
    let Some(name) = command.first() else {
        return false;
    };
    if name.eq_ignore_ascii_case(b"COMMAND") {
        return true;
    }
    if lookup(name).is_some() {
        return true;
    }
    let Some(subcommand) = command.get(1) else {
        return lookup(name).is_some();
    };
    let name = format!(
        "{}|{}",
        String::from_utf8_lossy(name).to_ascii_lowercase(),
        String::from_utf8_lossy(subcommand).to_ascii_lowercase()
    );
    lookup(name.as_bytes()).is_some()
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
            _ => true,
        },
    )
}

fn unknown_subcommand(name: &[u8]) -> Vec<u8> {
    let name = name
        .iter()
        .map(|byte| {
            if byte.is_ascii_graphic() && *byte != b'\'' {
                *byte as char
            } else {
                '?'
            }
        })
        .collect::<String>();
    protocol::error(&format!("unknown subcommand '{name}'. Try COMMAND HELP."))
}

pub fn execute(command: &[Vec<u8>]) -> Vec<u8> {
    let Some(subcommand) = command.get(1) else {
        return array(
            &root_commands()
                .iter()
                .map(|name| entry(name, true))
                .collect::<Vec<_>>(),
        );
    };
    if subcommand.eq_ignore_ascii_case(b"INFO") {
        let names: Vec<Option<String>> = if command.len() == 2 {
            root_commands().into_iter().map(Some).collect()
        } else {
            command[2..].iter().map(|name| lookup(name)).collect()
        };
        let infos = names
            .iter()
            .map(|name| {
                name.as_ref()
                    .map(|name| entry(name, !name.contains('|')))
                    .unwrap_or_else(|| b"$-1\r\n".to_vec())
            })
            .collect::<Vec<_>>();
        return array(&infos);
    }
    if subcommand.eq_ignore_ascii_case(b"COUNT") {
        if command.len() != 2 {
            return protocol::error("wrong number of arguments for 'command|count' command");
        }
        return format!(":{}\r\n", root_commands().len()).into_bytes();
    }
    unknown_subcommand(subcommand)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_table_matches_documented_inventory() {
        let inventory = include_str!("../docs/COMPATIBILITY.md");
        let command_table = inventory
            .lines()
            .skip_while(|line| !line.starts_with("| Area | Commands |"))
            .skip(1)
            .take_while(|line| line.starts_with("| "))
            .flat_map(|line| {
                let column = line.split('|').nth(2).unwrap_or_default();
                let mut names = Vec::new();
                let mut parts = column.split('`');
                while let (Some(_), Some(name)) = (parts.next(), parts.next()) {
                    names.push(
                        name.split_whitespace()
                            .map(str::to_ascii_lowercase)
                            .collect::<Vec<_>>()
                            .join("|"),
                    );
                }
                names
            })
            .collect::<BTreeSet<_>>();
        let metadata_table: BTreeSet<String> =
            COMMANDS.iter().map(|name| (*name).to_owned()).collect();
        assert_eq!(metadata_table, command_table);
        for name in COMMANDS {
            metadata(name);
        }
        for name in root_commands() {
            metadata(&name);
        }
    }

    #[test]
    fn redis_documented_arity_and_key_position_examples_are_correct() {
        for (name, expected) in [
            ("lmove", (5, 1, 2, 1)),
            ("linsert", (5, 1, 1, 1)),
            ("hexists", (3, 1, 1, 1)),
            ("zcount", (4, 1, 1, 1)),
            ("zrange", (-4, 1, 1, 1)),
            ("expire", (-3, 1, 1, 1)),
            ("config|set", (-4, 0, 0, 0)),
            ("quit", (-1, 0, 0, 0)),
            ("llen", (2, 1, 1, 1)),
            ("zintercard", (-3, 2, -1, 1)),
            ("hexpire", (-6, 1, 1, 1)),
            ("hpexpire", (-6, 1, 1, 1)),
            ("hexpireat", (-6, 1, 1, 1)),
            ("hpexpireat", (-6, 1, 1, 1)),
            ("httl", (-5, 1, 1, 1)),
            ("hpttl", (-5, 1, 1, 1)),
            ("hexpiretime", (-5, 1, 1, 1)),
            ("hpexpiretime", (-5, 1, 1, 1)),
            ("hpersist", (-5, 1, 1, 1)),
            ("hgetex", (-5, 1, 1, 1)),
            ("hsetex", (-6, 1, 1, 1)),
            ("rename", (3, 1, 2, 1)),
            ("renamenx", (3, 1, 2, 1)),
            ("copy", (-3, 1, 2, 1)),
            ("smove", (4, 1, 2, 1)),
            ("zrangestore", (-5, 1, 2, 1)),
            ("dbsize", (1, 0, 0, 0)),
            ("randomkey", (1, 0, 0, 0)),
            ("publish", (3, 0, 0, 0)),
        ] {
            let actual = metadata(name);
            assert_eq!(
                (
                    actual.arity,
                    actual.first_key,
                    actual.last_key,
                    actual.key_step
                ),
                expected,
                "{name}"
            );
        }
    }
}
