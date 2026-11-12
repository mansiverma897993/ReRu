// SPDX-License-Identifier: AGPL-3.0-only
#[path = "common/client.rs"]
mod client;
mod common;

use client::{array, integer, ok, queued, Client};

fn compare(ours: &mut Client, redis: &mut Client, command: &[&[u8]]) {
    let actual = ours.call(command);
    let expected = redis.call(command);
    assert_eq!(actual, expected, "reply mismatch for {command:?}");
}

#[test]
fn representative_resp2_sequences_match_a_real_redis_server() {
    let Ok(port) = std::env::var("RERU_REDIS_REFERENCE_PORT") else {
        return;
    };
    let port: u16 = port.parse().expect("reference port must be numeric");
    let server = common::Server::start();
    let mut ours = Client::new(&server);
    let mut redis = Client::on_port(port);
    for command in [
        vec![b"AUTH".as_slice(), b"unused"],
        vec![b"AUTH", b"default", b"unused"],
        vec![b"AUTH", b"Default", b"unused"],
        vec![b"ACL", b"WHOAMI"],
    ] {
        compare(&mut ours, &mut redis, &command);
    }
    let keys = [
        b"reru-diff:string".as_slice(),
        b"reru-diff:number",
        b"reru-diff:float",
        b"reru-diff:list",
        b"reru-diff:list2",
        b"reru-diff:hash",
        b"reru-diff:set",
        b"reru-diff:set2",
        b"reru-diff:zset",
        b"reru-diff:zlex",
        b"reru-diff:stream",
        b"reru-diff:copy",
        b"reru-diff:bitfield",
        b"reru-diff:list-more",
        b"reru-diff:list-more-dest",
        b"reru-diff:missing-list",
        b"reru-diff:zalg-a",
        b"reru-diff:zalg-b",
        b"reru-diff:zalg-out",
        b"reru-diff:zstore-src",
        b"reru-diff:zstore-lex",
        b"reru-diff:zstore-out",
        b"reru-diff:zstore-missing",
        b"reru-diff:zpop",
        b"reru-diff:zpop-missing",
        b"reru-diff:geo",
        b"reru-diff:geo-near",
    ];
    let mut reset = vec![b"DEL".as_slice()];
    reset.extend(keys);
    let _ = redis.call(&reset);

    for command in [
        &[b"SET".as_slice(), b"reru-diff:string", b"ab"] as &[&[u8]],
        &[b"GET", b"reru-diff:string"],
        &[b"SETNX", b"reru-diff:string", b"ignored"],
        &[b"GETSET", b"reru-diff:string", b"abc"],
        &[b"APPEND", b"reru-diff:string", b"de"],
        &[b"STRLEN", b"reru-diff:string"],
        &[b"GETRANGE", b"reru-diff:string", b"-3", b"-1"],
        &[b"SUBSTR", b"reru-diff:string", b"-3", b"-1"],
        &[b"SETRANGE", b"reru-diff:string", b"1", b"Z"],
        &[b"GET", b"reru-diff:string"],
        &[b"INCRBY", b"reru-diff:number", b"5"],
        &[b"DECR", b"reru-diff:number"],
        &[b"DECRBY", b"reru-diff:number", b"-3"],
        &[b"GET", b"reru-diff:number"],
        &[b"INCRBYFLOAT", b"reru-diff:float", b"10.5"],
        &[b"INCRBYFLOAT", b"reru-diff:float", b"0.1"],
        &[b"GET", b"reru-diff:float"],
        &[b"TYPE", b"reru-diff:string"],
        &[b"COPY", b"reru-diff:string", b"reru-diff:copy"],
        &[b"GET", b"reru-diff:copy"],
        &[b"PERSIST", b"reru-diff:string"],
    ] {
        compare(&mut ours, &mut redis, command);
    }

    for command in [
        &[b"RPUSH".as_slice(), b"reru-diff:list", b"a", b"b", b"c"] as &[&[u8]],
        &[b"LPUSH", b"reru-diff:list", b"first"],
        &[b"LLEN", b"reru-diff:list"],
        &[b"LINDEX", b"reru-diff:list", b"-1"],
        &[b"LSET", b"reru-diff:list", b"1", b"A"],
        &[b"LRANGE", b"reru-diff:list", b"0", b"-1"],
        &[b"LPOS", b"reru-diff:list", b"A"],
        &[b"LREM", b"reru-diff:list", b"1", b"b"],
        &[b"RPOP", b"reru-diff:list"],
        &[
            b"LMOVE",
            b"reru-diff:list",
            b"reru-diff:list2",
            b"RIGHT",
            b"LEFT",
        ],
        &[b"BRPOP", b"reru-diff:list2", b"0.1"],
        &[b"LRANGE", b"reru-diff:list", b"0", b"-1"],
        &[b"LRANGE", b"reru-diff:list2", b"0", b"-1"],
    ] {
        compare(&mut ours, &mut redis, command);
    }

    for command in [
        &[
            b"RPUSH".as_slice(),
            b"reru-diff:list-more",
            b"a",
            b"b",
            b"c",
            b"d",
        ] as &[&[u8]],
        &[
            b"LMPOP",
            b"2",
            b"reru-diff:missing-list",
            b"reru-diff:list-more",
            b"LEFT",
            b"COUNT",
            b"2",
        ],
        &[
            b"BLMOVE",
            b"reru-diff:list-more",
            b"reru-diff:list-more-dest",
            b"RIGHT",
            b"LEFT",
            b"0.05",
        ],
        &[
            b"BRPOPLPUSH",
            b"reru-diff:list-more",
            b"reru-diff:list-more-dest",
            b"0.05",
        ],
        &[b"BLMPOP", b"0.01", b"1", b"reru-diff:list-more", b"RIGHT"],
        &[b"LRANGE", b"reru-diff:list-more-dest", b"0", b"-1"],
    ] {
        compare(&mut ours, &mut redis, command);
    }

    for command in [
        &[
            b"HSET".as_slice(),
            b"reru-diff:hash",
            b"a",
            b"10",
            b"b",
            b"hi",
        ] as &[&[u8]],
        &[b"HGET", b"reru-diff:hash", b"b"],
        &[b"HSTRLEN", b"reru-diff:hash", b"b"],
        &[b"HINCRBY", b"reru-diff:hash", b"a", b"-4"],
        &[b"HINCRBYFLOAT", b"reru-diff:hash", b"a", b"0.5"],
        &[b"HMGET", b"reru-diff:hash", b"a", b"b", b"missing"],
        &[b"HEXISTS", b"reru-diff:hash", b"missing"],
        &[b"HLEN", b"reru-diff:hash"],
        &[b"HDEL", b"reru-diff:hash", b"b"],
    ] {
        compare(&mut ours, &mut redis, command);
    }

    for command in [
        &[
            b"ZADD".as_slice(),
            b"reru-diff:zpop",
            b"1",
            b"a",
            b"2",
            b"b",
            b"3",
            b"c",
        ] as &[&[u8]],
        &[
            b"ZMPOP",
            b"2",
            b"reru-diff:zpop-missing",
            b"reru-diff:zpop",
            b"MIN",
            b"COUNT",
            b"2",
        ],
        &[b"BZMPOP", b"0.05", b"1", b"reru-diff:zpop", b"MAX"],
        &[b"BZPOPMIN", b"reru-diff:zpop", b"0.01"],
        &[b"ZADD", b"reru-diff:zpop", b"4", b"last"],
        &[b"BZPOPMAX", b"reru-diff:zpop", b"0.05"],
    ] {
        compare(&mut ours, &mut redis, command);
    }

    for command in [
        &[
            b"GEOADD".as_slice(),
            b"reru-diff:geo",
            b"13.361389",
            b"38.115556",
            b"Palermo",
            b"15.087269",
            b"37.502669",
            b"Catania",
        ] as &[&[u8]],
        &[b"ZSCORE", b"reru-diff:geo", b"Palermo"],
        &[b"GEOPOS", b"reru-diff:geo", b"Palermo", b"Catania"],
        &[b"GEOHASH", b"reru-diff:geo", b"Palermo", b"Catania"],
        &[b"GEODIST", b"reru-diff:geo", b"Palermo", b"Catania", b"km"],
        &[
            b"GEOADD",
            b"reru-diff:geo",
            b"XX",
            b"CH",
            b"13.5",
            b"38.2",
            b"Palermo",
        ],
        &[
            b"GEOSEARCH",
            b"reru-diff:geo",
            b"FROMMEMBER",
            b"Palermo",
            b"BYRADIUS",
            b"200",
            b"km",
            b"ASC",
            b"WITHDIST",
            b"WITHHASH",
            b"WITHCOORD",
        ],
        &[
            b"GEOSEARCH",
            b"reru-diff:geo",
            b"FROMLONLAT",
            b"13",
            b"38",
            b"BYBOX",
            b"500",
            b"500",
            b"km",
            b"ASC",
            b"COUNT",
            b"1",
        ],
        &[
            b"GEOSEARCHSTORE",
            b"reru-diff:geo-near",
            b"reru-diff:geo",
            b"FROMMEMBER",
            b"Palermo",
            b"BYRADIUS",
            b"200",
            b"km",
            b"ASC",
            b"STOREDIST",
        ],
        &[b"ZSCORE", b"reru-diff:geo-near", b"Catania"],
    ] {
        compare(&mut ours, &mut redis, command);
    }

    for command in [
        &[b"SADD".as_slice(), b"reru-diff:set", b"a", b"b", b"c"] as &[&[u8]],
        &[b"SADD", b"reru-diff:set2", b"b", b"d"],
        &[b"SISMEMBER", b"reru-diff:set", b"a"],
        &[b"SMISMEMBER", b"reru-diff:set", b"a", b"missing", b"b"],
        &[b"SCARD", b"reru-diff:set"],
        &[b"SMOVE", b"reru-diff:set", b"reru-diff:set2", b"a"],
        &[b"SISMEMBER", b"reru-diff:set2", b"a"],
        &[b"SREM", b"reru-diff:set2", b"d"],
        &[b"SINTERCARD", b"2", b"reru-diff:set", b"reru-diff:set2"],
        &[
            b"SINTERCARD",
            b"2",
            b"reru-diff:set",
            b"reru-diff:set2",
            b"LIMIT",
            b"1",
        ],
    ] {
        compare(&mut ours, &mut redis, command);
    }

    for command in [
        &[
            b"ZADD".as_slice(),
            b"reru-diff:zset",
            b"1",
            b"a",
            b"2",
            b"b",
            b"3",
            b"c",
        ] as &[&[u8]],
        &[b"ZRANGE", b"reru-diff:zset", b"0", b"-1", b"WITHSCORES"],
        &[b"ZMSCORE", b"reru-diff:zset", b"c", b"missing", b"a"],
        &[b"ZRANGEBYSCORE", b"reru-diff:zset", b"(1", b"+inf"],
        &[b"ZRANGE", b"reru-diff:zset", b"3", b"1", b"BYSCORE", b"REV"],
        &[b"ZCOUNT", b"reru-diff:zset", b"-inf", b"2"],
        &[b"ZPOPMAX", b"reru-diff:zset", b"1"],
        &[b"ZREMRANGEBYRANK", b"reru-diff:zset", b"0", b"0"],
        &[b"ZCARD", b"reru-diff:zset"],
    ] {
        compare(&mut ours, &mut redis, command);
    }

    for command in [
        &[
            b"ZADD".as_slice(),
            b"reru-diff:zlex",
            b"0",
            b"a",
            b"0",
            b"b",
            b"0",
            b"c",
        ] as &[&[u8]],
        &[b"ZLEXCOUNT", b"reru-diff:zlex", b"[a", b"(c"],
        &[
            b"ZRANGEBYLEX",
            b"reru-diff:zlex",
            b"-",
            b"+",
            b"LIMIT",
            b"1",
            b"1",
        ],
        &[
            b"ZREVRANGEBYLEX",
            b"reru-diff:zlex",
            b"+",
            b"-",
            b"LIMIT",
            b"0",
            b"2",
        ],
        &[
            b"ZRANGE",
            b"reru-diff:zlex",
            b"[a",
            b"+",
            b"BYLEX",
            b"WITHSCORES",
        ],
        &[b"ZREMRANGEBYLEX", b"reru-diff:zlex", b"[b", b"+"],
        &[b"ZRANGE", b"reru-diff:zlex", b"0", b"-1"],
    ] {
        compare(&mut ours, &mut redis, command);
    }

    for command in [
        &[
            b"ZADD".as_slice(),
            b"reru-diff:zstore-src",
            b"1",
            b"a",
            b"2",
            b"b",
            b"2",
            b"c",
            b"3",
            b"d",
        ] as &[&[u8]],
        &[
            b"ZADD",
            b"reru-diff:zstore-lex",
            b"0",
            b"a",
            b"0",
            b"b",
            b"0",
            b"c",
            b"0",
            b"d",
        ],
        &[
            b"ZRANGESTORE",
            b"reru-diff:zstore-out",
            b"reru-diff:zstore-src",
            b"1",
            b"2",
        ],
        &[
            b"ZRANGE",
            b"reru-diff:zstore-out",
            b"0",
            b"-1",
            b"WITHSCORES",
        ],
        &[
            b"ZRANGESTORE",
            b"reru-diff:zstore-out",
            b"reru-diff:zstore-src",
            b"0",
            b"1",
            b"REV",
        ],
        &[b"ZRANGE", b"reru-diff:zstore-out", b"0", b"-1", b"REV"],
        &[
            b"ZRANGESTORE",
            b"reru-diff:zstore-out",
            b"reru-diff:zstore-src",
            b"+inf",
            b"-inf",
            b"BYSCORE",
            b"REV",
            b"LIMIT",
            b"0",
            b"2",
        ],
        &[
            b"ZRANGE",
            b"reru-diff:zstore-out",
            b"0",
            b"-1",
            b"WITHSCORES",
        ],
        &[
            b"ZRANGESTORE",
            b"reru-diff:zstore-out",
            b"reru-diff:zstore-lex",
            b"-",
            b"+",
            b"BYLEX",
            b"LIMIT",
            b"1",
            b"2",
        ],
        &[b"ZRANGE", b"reru-diff:zstore-out", b"0", b"-1"],
        &[
            b"ZRANGESTORE",
            b"reru-diff:zstore-out",
            b"reru-diff:zstore-src",
            b"5",
            b"9",
        ],
        &[b"EXISTS", b"reru-diff:zstore-out"],
        &[
            b"ZRANGESTORE",
            b"reru-diff:zstore-out",
            b"reru-diff:zstore-src",
            b"0",
            b"-1",
            b"LIMIT",
            b"0",
            b"1",
        ],
        &[
            b"ZRANGESTORE",
            b"reru-diff:zstore-out",
            b"reru-diff:zstore-src",
            b"0",
            b"-1",
            b"WITHSCORES",
        ],
        &[
            b"ZRANGESTORE",
            b"reru-diff:zstore-out",
            b"reru-diff:zstore-src",
            b"0",
        ],
        &[
            b"ZRANGESTORE",
            b"reru-diff:zstore-out",
            b"reru-diff:zstore-missing",
            b"0",
            b"-1",
        ],
        &[
            b"ZRANGE",
            b"reru-diff:zstore-src",
            b"-inf",
            b"+inf",
            b"BYSCORE",
            b"LIMIT",
            b"-1",
            b"2",
        ],
        &[
            b"ZRANGE",
            b"reru-diff:zstore-lex",
            b"-",
            b"+",
            b"BYLEX",
            b"LIMIT",
            b"-1",
            b"2",
        ],
        &[
            b"ZRANGESTORE",
            b"reru-diff:zstore-out",
            b"reru-diff:zstore-src",
            b"-inf",
            b"+inf",
            b"BYSCORE",
            b"LIMIT",
            b"-1",
            b"2",
        ],
        &[b"ZRANGE", b"reru-diff:zstore-src", b"bad", b"3", b"BYSCORE"],
        &[b"ZCOUNT", b"reru-diff:zstore-src", b"bad", b"3"],
        &[b"ZREMRANGEBYSCORE", b"reru-diff:zstore-src", b"bad", b"3"],
        &[b"ZRANGE", b"reru-diff:zstore-src", b"", b"2", b"BYSCORE"],
    ] {
        compare(&mut ours, &mut redis, command);
    }

    for command in [
        &[
            b"ZADD".as_slice(),
            b"reru-diff:zalg-a",
            b"1",
            b"one",
            b"2",
            b"two",
        ] as &[&[u8]],
        &[b"ZADD", b"reru-diff:zalg-b", b"3", b"one", b"4", b"three"],
        &[
            b"ZUNION",
            b"2",
            b"reru-diff:zalg-a",
            b"reru-diff:zalg-b",
            b"WITHSCORES",
        ],
        &[
            b"ZUNION",
            b"2",
            b"reru-diff:zalg-a",
            b"reru-diff:set",
            b"WITHSCORES",
        ],
        &[
            b"ZINTER",
            b"2",
            b"reru-diff:zalg-a",
            b"reru-diff:zalg-b",
            b"WEIGHTS",
            b"2",
            b"3",
            b"AGGREGATE",
            b"MAX",
            b"WITHSCORES",
        ],
        &[b"ZDIFF", b"2", b"reru-diff:zalg-a", b"reru-diff:zalg-b"],
        &[
            b"ZINTERCARD",
            b"2",
            b"reru-diff:zalg-a",
            b"reru-diff:zalg-b",
            b"LIMIT",
            b"1",
        ],
        &[
            b"ZUNIONSTORE",
            b"reru-diff:zalg-out",
            b"2",
            b"reru-diff:zalg-a",
            b"reru-diff:zalg-b",
        ],
        &[b"ZRANGE", b"reru-diff:zalg-out", b"0", b"-1", b"WITHSCORES"],
        &[
            b"ZINTERSTORE",
            b"reru-diff:zalg-out",
            b"2",
            b"reru-diff:zalg-a",
            b"reru-diff:zalg-b",
        ],
        &[
            b"ZDIFFSTORE",
            b"reru-diff:zalg-out",
            b"2",
            b"reru-diff:zalg-a",
            b"reru-diff:zalg-b",
        ],
    ] {
        compare(&mut ours, &mut redis, command);
    }

    for command in [
        &[
            b"BITFIELD".as_slice(),
            b"reru-diff:bitfield",
            b"GET",
            b"i8",
            b"#0",
        ] as &[&[u8]],
        &[
            b"BITFIELD",
            b"reru-diff:bitfield",
            b"SET",
            b"i8",
            b"#0",
            b"127",
        ],
        &[
            b"BITFIELD",
            b"reru-diff:bitfield",
            b"INCRBY",
            b"i8",
            b"#0",
            b"1",
        ],
        &[b"BITFIELD_RO", b"reru-diff:bitfield", b"GET", b"i8", b"#0"],
        &[
            b"BITFIELD",
            b"reru-diff:bitfield",
            b"OVERFLOW",
            b"SAT",
            b"INCRBY",
            b"i8",
            b"#0",
            b"-10",
            b"GET",
            b"u4",
            b"4",
        ],
        &[
            b"BITFIELD",
            b"reru-diff:bitfield",
            b"OVERFLOW",
            b"FAIL",
            b"INCRBY",
            b"u2",
            b"#4",
            b"-1",
        ],
        &[
            b"BITFIELD",
            b"reru-diff:bitfield",
            b"OVERFLOW",
            b"SAT",
            b"SET",
            b"u2",
            b"#4",
            b"9",
        ],
        &[
            b"BITFIELD",
            b"reru-diff:bitfield",
            b"OVERFLOW",
            b"FAIL",
            b"INCRBY",
            b"u2",
            b"#4",
            b"1",
        ],
        &[
            b"BITFIELD",
            b"reru-diff:bitfield",
            b"SET",
            b"u5",
            b"7",
            b"23",
        ],
        &[b"GET", b"reru-diff:bitfield"],
        &[b"BITFIELD_RO", b"reru-diff:bitfield", b"GET", b"u5", b"7"],
    ] {
        compare(&mut ours, &mut redis, command);
    }

    for command in [
        &[b"XADD".as_slice(), b"reru-diff:stream", b"1-0", b"f", b"a"] as &[&[u8]],
        &[b"XADD", b"reru-diff:stream", b"1-1", b"f", b"b"],
        &[b"XADD", b"reru-diff:stream", b"2-0", b"f", b"c"],
        &[b"XRANGE", b"reru-diff:stream", b"-", b"+"],
        &[
            b"XREVRANGE",
            b"reru-diff:stream",
            b"+",
            b"-",
            b"COUNT",
            b"2",
        ],
        &[b"XREVRANGE", b"reru-diff:stream", b"(2-0", b"1-0"],
        &[b"XDEL", b"reru-diff:stream", b"2-0"],
        &[b"XTRIM", b"reru-diff:stream", b"MAXLEN", b"0"],
        &[b"XLEN", b"reru-diff:stream"],
        &[b"TYPE", b"reru-diff:stream"],
        &[b"XADD", b"reru-diff:stream", b"2-*", b"f", b"d"],
    ] {
        compare(&mut ours, &mut redis, command);
    }

    assert_eq!(ours.call(&[b"EXISTS", b"reru-diff:copy"]), integer(1));
    assert_eq!(redis.call(&[b"EXISTS", b"reru-diff:copy"]), integer(1));
    assert_eq!(ours.call(&[b"MULTI"]), ok());
    assert_eq!(redis.call(&[b"MULTI"]), ok());
    assert_eq!(
        ours.call(&[b"SET", b"reru-diff:copy", b"transaction"]),
        queued()
    );
    assert_eq!(
        redis.call(&[b"SET", b"reru-diff:copy", b"transaction"]),
        queued()
    );
    assert_eq!(ours.call(&[b"EXEC"]), array(vec![ok()]));
    assert_eq!(redis.call(&[b"EXEC"]), array(vec![ok()]));
    let _ = redis.call(&reset);
}
