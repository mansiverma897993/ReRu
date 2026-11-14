# ReRu compatibility status

Updated: 14 November 2026. This inventory describes the implementation and local
tests; it does not certify complete compatibility with Redis.

## Implemented commands

| Area | Commands | Current scope |
| --- | --- | --- |
| Connection | `PING`, `ECHO`, `HELLO`, `SELECT`, `RESET`, `CLIENT ID`, `CLIENT SETNAME`, `CLIENT GETNAME`, `CLIENT SETINFO`, `CLIENT INFO` | RESP2 handshake, optional inline authentication/name, database 0 selection, socket metadata and context reset |
| Strings/cache | `SET`, `GET`, `INCR`, `GETEX`, `GETDEL`, `MGET`, `MSET`, `MSETNX`, `APPEND`, `STRLEN`, `SETNX`, `SETEX`, `PSETEX`, `GETSET`, `INCRBY`, `INCRBYFLOAT`, `DECR`, `DECRBY`, `GETRANGE`, `SUBSTR`, `SETRANGE` | Binary values; SET NX/XX/GET/EX/PX/EXAT/PXAT/KEEPTTL; GETEX expiry/PERSIST; multi-key access, numeric arithmetic and byte ranges |
| Lists | `RPUSH`, `LPUSH`, `LPUSHX`, `RPUSHX`, `LRANGE`, `LLEN`, `LINDEX`, `LSET`, `LINSERT`, `LTRIM`, `LREM`, `LPOS`, `LPOP`, `RPOP`, `RPOPLPUSH`, `LMOVE`, `BLPOP`, `BRPOP`, `BLMOVE`, `BRPOPLPUSH`, `LMPOP`, `BLMPOP` | Multiple elements, negative indexes, edits, moves, position search, single/multi-key blocking pops and moves with timeout |
| Streams | `XADD`, `XRANGE`, `XREVRANGE`, `XREAD`, `XLEN`, `XDEL`, `XTRIM`, `XGROUP CREATE`, `XGROUP SETID`, `XGROUP DESTROY`, `XGROUP CREATECONSUMER`, `XGROUP DELCONSUMER`, `XREADGROUP`, `XACK`, `XPENDING`, `XCLAIM`, `XAUTOCLAIM`, `XINFO GROUPS`, `XINFO CONSUMERS`, `XINFO STREAM`, `XSETID` | IDs, ranges, deletion, exact trimming, blocking reads, consumer groups, pending entries, acknowledgement, claiming and diagnostics; XADD NOMKSTREAM/MAXLEN/MINID |
| Hashes | `HSET`, `HMSET`, `HSETNX`, `HGET`, `HMGET`, `HGETALL`, `HDEL`, `HEXISTS`, `HLEN`, `HKEYS`, `HVALS`, `HINCRBY`, `HINCRBYFLOAT`, `HSTRLEN`, `HRANDFIELD`, `HSCAN`, `HEXPIRE`, `HPEXPIRE`, `HEXPIREAT`, `HPEXPIREAT`, `HTTL`, `HPTTL`, `HEXPIRETIME`, `HPEXPIRETIME`, `HPERSIST`, `HGETEX`, `HSETEX` | Binary fields/values, multiple field updates, missing-field nil replies, increments, cursor iteration, per-field expiry and conditional TTL changes |
| Sets | `SADD`, `SREM`, `SISMEMBER`, `SMISMEMBER`, `SMEMBERS`, `SCARD`, `SMOVE`, `SPOP`, `SRANDMEMBER`, `SUNION`, `SINTER`, `SINTERCARD`, `SDIFF`, `SUNIONSTORE`, `SINTERSTORE`, `SDIFFSTORE`, `SSCAN` | Binary members, deduplication, membership, cardinality, random selection, atomic moves, set algebra and cursor iteration; SINTERCARD LIMIT |
| Sorted sets | `ZADD`, `ZINCRBY`, `ZCARD`, `ZSCORE`, `ZRANK`, `ZREVRANK`, `ZRANGE`, `ZREVRANGE`, `ZRANGESTORE`, `ZRANGEBYSCORE`, `ZREVRANGEBYSCORE`, `ZRANGEBYLEX`, `ZREVRANGEBYLEX`, `ZCOUNT`, `ZLEXCOUNT`, `ZREM`, `ZREMRANGEBYSCORE`, `ZREMRANGEBYRANK`, `ZREMRANGEBYLEX`, `ZMSCORE`, `ZSCAN`, `ZPOPMIN`, `ZPOPMAX`, `ZMPOP`, `BZMPOP`, `BZPOPMIN`, `BZPOPMAX`, `ZUNION`, `ZUNIONSTORE`, `ZINTER`, `ZINTERSTORE`, `ZDIFF`, `ZDIFFSTORE`, `ZINTERCARD` | Score-ordered members, rank, score and lex ranges, ZADD NX/XX/GT/LT/CH/INCR, cursor iteration, range removal, ordered and multi-key blocking pops, weighted algebra and destination storing |
| Bitmaps | `SETBIT`, `GETBIT`, `BITCOUNT`, `BITOP`, `BITPOS`, `BITFIELD`, `BITFIELD_RO` | Bitmaps stored as strings, byte/bit count ranges, AND/OR/XOR/NOT; bitfield GET/SET/INCRBY, signed/unsigned widths, `#` offsets and WRAP/SAT/FAIL overflow modes |
| HyperLogLog | `PFADD`, `PFCOUNT`, `PFMERGE` | Approximate distinct counts, multi-key unions, binary members, Redis dense/sparse interchange, cached cardinality |
| Geospatial | `GEOADD`, `GEOPOS`, `GEOHASH`, `GEODIST`, `GEOSEARCH`, `GEOSEARCHSTORE` | Geo points use sorted-set scores; NX/XX/CH, positions and distances, radius/box search, sort/count, decorated results, and destination storing |
| Keyspace | `TYPE`, `DEL`, `UNLINK`, `EXISTS`, `TOUCH`, `DBSIZE`, `KEYS`, `SCAN`, `RANDOMKEY`, `COPY`, `RENAME`, `RENAMENX`, `TTL`, `PTTL`, `EXPIRE`, `PEXPIRE`, `EXPIREAT`, `PEXPIREAT`, `EXPIRETIME`, `PEXPIRETIME`, `PERSIST`, `FLUSHDB`, `FLUSHALL` | Multiple keys, glob and type filtered scanning, rename/copy with TTL, relative/absolute expiry conditions, whole-database flush |
| Transactions | `MULTI`, `EXEC`, `DISCARD`, `WATCH`, `UNWATCH` | Queuing, atomic execution relative to other commands, runtime errors, optimistic locking |
| Pub/Sub | `PUBLISH`, `SUBSCRIBE`, `UNSUBSCRIBE`, `PSUBSCRIBE`, `PUNSUBSCRIBE`, `PUBSUB` | Binary channels/messages, glob patterns, RESP2 subscribed mode, CHANNELS/NUMSUB/NUMPAT; local process only |
| Connection close | `QUIT` | Replies OK and closes the connection |
| Authentication | `AUTH`, `ACL WHOAMI` | Startup-configured password for the default user, connection-local authentication and authenticated ReRu replication |
| Configuration | `CONFIG GET`, `CONFIG SET`, [`CONFIG RESETSTAT`](https://redis.io/docs/latest/commands/config-resetstat/) | Runtime `maxmemory`, `maxmemory-policy`, and `maxclients`; GET also reports AOF enabled/fsync mode and configured RDB dir/dbfilename; RESETSTAT clears counters exposed by INFO |
| Observability | `INFO` | Partial `memory`, `stats`, `clients`, and `persistence` sections |
| Server time | `TIME` | Epoch seconds and microseconds as RESP2 bulk strings |
| Persistence | `SAVE`, `BGSAVE`, `LASTSAVE`, `DUMP`, `RESTORE`, `BGREWRITEAOF` | Initial Redis RDB interoperability; supported values, key expiry, checksums, REPLACE/ABSTTL; manual single-file AOF compaction |
| Command introspection | [`COMMAND`](https://redis.io/docs/latest/commands/command/), `COMMAND COUNT`, `COMMAND INFO` | Lists ReRu's implemented command metadata, counts entries, or looks up names |

`COMMAND` entries use the ten-field Redis 8 reply shape: name, arity, flags,
first/last/step key positions, ACL categories, tips, key specifications, and
subcommands. ReRu does not track command flags, ACL categories, tips, or key
specifications, so those fields are empty arrays. Commands whose keys are
selected by a `STREAMS` marker (`XREAD` and `XREADGROUP`) report zero key
positions. Subcommands are nested under their parent command. `COMMAND INFO`
with no names returns all entries; named lookups preserve order and return nil
for unknown names. `COMMAND` and `COMMAND INFO` return the same top-level
entries, and `COMMAND COUNT` counts those entries. Redis documents
[`COMMAND`](https://redis.io/docs/latest/commands/command/),
[`COMMAND COUNT`](https://redis.io/docs/latest/commands/command-count/), and
[`COMMAND INFO`](https://redis.io/docs/latest/commands/command-info/).

Total: **219 Redis command metadata entries**, or **47.7% name presence**
against 459 entries in the pinned Redis 8.10.2 metadata. This is neither a
behavior pass rate nor a percentage of Redis completed. Run
`powershell -File scripts/command_coverage.ps1 -RedisSource PATH_TO_REDIS_8_10_2`
from the repository root to reproduce the count. Many options and error cases
within those entries remain unsupported.

`LMPOP` and `BLMPOP` choose the first nonempty key in argument order and return
the popped members in left or right pop order. `BLMOVE` and `BRPOPLPUSH` move
one element atomically and block on a missing source. Timeout `0` waits
indefinitely outside transactions; inside MULTI/EXEC these commands return
immediately. Waiting clients do not yet have Redis's FIFO ordering guarantee.
`ZMPOP`, `BZMPOP`, `BZPOPMIN`, and `BZPOPMAX` similarly choose the first
nonempty sorted-set key, pop in score order, and delete an emptied key.
Blocking forms wait for another write and return immediately inside MULTI/EXEC.

Geo search scans the sorted set, then filters and sorts candidates. This is
slower than Redis's geohash-area query for large datasets. `COUNT ANY` currently
uses the same post-filter limit as `COUNT`; Redis may stop early. The
`GEORADIUS` aliases and cross-dateline box behavior remain to be verified.

WATCH tracks writes even if the resulting value is unchanged, missing-key
creation/deletion, and natural expiry. EXEC and DISCARD clear the connection's
watches; UNWATCH clears them outside MULTI. Inside MULTI, UNWATCH is queued.
Disconnects release registrations. Replica updates identify changed keys;
unrelated writes preserve watches, while a full resync invalidates all watches.

Hashes and sets preserve key expiry when updated and disappear after the last
field/member is removed. No-op HSETNX, HDEL, SADD and SREM preserve watches;
HSET/HMSET writes invalidate watches even when values are unchanged. Hash/set
enumeration is sorted in ReRu for stable output; clients should treat ordering
as unspecified. HMSET is supported for older clients; prefer HSET for new code.
Hash fields can also expire independently. The four H*EXPIRE variants accept
NX/XX/GT/LT and report a status for each field; HTTL/HPTTL and the absolute-time
queries report -2 for missing fields and -1 for persistent fields. HGETEX and
HSETEX support their Redis 8 expiry, persistence, and conditional options. A
field's TTL is cleared by HSET/HMSET/HDEL and preserved by HINCRBY/HINCRBYFLOAT.
Expired fields disappear on reads and through the background expiry worker,
which indexes each hash by its next field deadline. Field deadlines survive ReRu
snapshots, AOF replay and rewrite, and ReRu-to-ReRu replication. Redis RDB
SAVE/BGSAVE and DUMP **reject hashes with live field TTLs** rather than silently
dropping them; Redis RDB hash-field expiry encoding and RESTORE interoperability
remain pending. Commands and error replies are covered by local integration tests
and a Redis 8.10.2 differential test.
`SINTERCARD` supports multiple keys and `LIMIT`. `SPOP` and `SRANDMEMBER` use a randomized member order; count replies are capped
at one million elements to bound allocation. Set store variants replace the
destination and clear its previous TTL.
`HRANDFIELD` uses the same one-million count cap. `HINCRBYFLOAT` uses Rust `f64`
arithmetic, so last-digit results can differ from Redis long-double arithmetic.
`INCRBYFLOAT` has the same precision limitation. Both reject nonfinite input
and overflow without changing the stored value.
`HSCAN`, `SSCAN`, and `ZSCAN` currently use sorted offset cursors, rebuilding
their index for each call. They apply `MATCH` after page selection and accept
`COUNT` as a page size; concurrent edits may repeat or skip elements. `HSCAN`
supports `NOVALUES`. `SETRANGE` currently caps growth at 64 MiB.

## Memory limit and eviction

`--maxmemory SIZE` accepts bytes or `k`/`kb`, `m`/`mb`, and `g`/`gb` suffixes.
The default `0` means unlimited. `--maxmemory-policy` accepts `noeviction`,
`allkeys-lru`, `allkeys-lrm`, `allkeys-lfu`, `allkeys-random`, `volatile-lru`,
`volatile-lrm`, `volatile-lfu`, `volatile-random`, and `volatile-ttl`. ReRu
estimates logical dataset bytes from keys, values, and fixed per-entry overhead;
this is **not** Redis allocator memory, process RSS, or an exact RAM ceiling.
Allkeys policies may evict any live key. Volatile policies only evict live keys
with expiry; a growing write returns OOM and rolls back if those candidates
cannot free enough space. `noeviction` also rolls back growing writes on OOM,
including their WATCH effects. Evictions invalidate WATCH and are included in
ReRu snapshots and replication. Loaded snapshots are trimmed to the limit when
an allkeys policy is selected; recency and frequency are not persisted.

LRU and LRM use exact in-memory order; LFU uses a simplified frequency counter
with minute-based decay. Redis uses sampled eviction and a different LFU
algorithm. Random policies pick from a shuffled candidate order, and
`volatile-ttl` picks the earliest expiry. Memory accounting scans the dataset;
bounded writes under noeviction and volatile policies copy it for rollback.
These paths need further performance work. `INFO memory` reports the logical
estimate as `used_memory` and `reru_estimated_dataset_bytes`; `INFO stats`
reports `evicted_keys`. Additional supported fields are described below.
`CONFIG GET/SET` supports the two memory settings and `maxclients`; runtime changes are not
saved to a config file.

### Background expiry and connection limits

Primaries reclaim idle expired keys using a deadline index with one record per
expiring key. A worker attempts a cycle every 100 ms, processing up to 256 keys
or 5 ms of index/deletion work while holding the command gate. Persistence and
replication serialization are outside this time budget and can take longer.
Busy commands can delay expiry. TTL changes replace the previous index entry;
PERSIST, replacement, deletion and eviction remove it. Startup rebuilds the index.
Deletion invalidates WATCH and follows the normal AOF/snapshot/replication path.
Replicas receive primary deletions and do not run an independent expiry worker.
This differs from Redis's sampled active-expiry algorithm.

`INFO memory` exposes `reru_retained_keys` (including expired entries not yet
reclaimed) and `reru_expiry_index_keys`. `INFO stats` exposes
`reru_active_expired_keys`, counting only keys physically removed by the worker.
The index's memory overhead is not included in ReRu's logical maxmemory estimate.

`--maxclients COUNT` defaults to 10000 and requires a positive integer. Accepted
connections, including replication and subscriptions, hold a slot until their
handler exits. Excess clients receive `ERR max number of clients reached`.
Blocking list, sorted-set and stream commands probe their TCP connection while
waiting; a closed connection releases its client slot without waiting for a key
change or command timeout. The probe does not consume pipelined request bytes.
An OS-unreported half-open connection cannot be detected until TCP reports it.
Lowering `CONFIG SET maxclients` keeps existing connections and rejects new ones
until capacity becomes available. `INFO clients` reports `connected_clients`
and `maxclients`; `INFO stats` adds `total_connections_received` and
`rejected_connections`. Thread creation failures are logged. These limits do not
bound transaction queues, Pub/Sub buffers, aggregate request memory or RSS.

List counts are validated the way Redis validates them: an unusable count for
`LPOP` or `RPOP`, and an unusable `RANK`, `COUNT` or `MAXLEN` for `LPOS`, return
Redis's error text, and a rejected count does not touch the key. `BLMOVE` and
`BRPOPLPUSH` return a nil array when their timeout expires, and a nil bulk inside
`MULTI`, where they run without blocking.

`COPY` supports same-database copy and `REPLACE`; `DB` selection is pending.
`FLUSHDB` and `FLUSHALL` both clear ReRu's only database. `ASYNC` is accepted
but deletion is currently synchronous; flush invalidates WATCH and is persisted
through ReRu snapshots/AOF.
`UNLINK` removes keys immediately but frees memory synchronously, unlike
Redis's background reclamation. `DBSIZE` counts unexpired keys; the separate
expiry worker reclaims idle keys. `TOUCH` updates the in-memory access order and frequency.
`KEYS` and `SCAN` exclude expired keys. `SCAN` uses a sorted index offset as
its cursor, applies `MATCH` and `TYPE` after selecting each page, and treats
`COUNT` as a page-size hint. It rebuilds and sorts the live key list on every
call, so large keyspaces need a more scalable cursor implementation. Changes
to the keyspace during a scan can shift the index and repeat or skip keys.

Sorted sets keep a member lookup map and a score-ordered index. Score changes
update both indexes. Rank lookup is still linear in the number of members;
range scans advance through the ordered index. `ZRANGE` supports rank indexes,
`BYSCORE`, `BYLEX`, `REV`, `WITHSCORES`, and `LIMIT` on applicable ranges.
`ZRANGESTORE` reuses that selection, replaces the destination with the result,
clears its TTL, and deletes it when nothing matches. `LIMIT` without `BYSCORE` or
`BYLEX` is rejected with the same error Redis returns; options may appear in any
order. A negative `LIMIT` offset selects nothing and a negative count means all,
both as in Redis. Score bounds report `ERR min or max is not a float`, and an
empty bound reads as zero; `ZADD` and `ZINCRBY` still reject an empty or
unparsable score with `ERR value is not a valid float`. Hex-float literals such
as `0x1`, which Redis accepts through `strtod`, are rejected everywhere.
Lex ranges use the score-ordered index and should be used with equal scores,
the ordering condition documented by Redis. Bitmap growth is capped at 64 MiB per value, below Redis's
maximum; this is an explicit ReRu resource limit.
`BITFIELD` and `BITFIELD_RO` support integer widths up to `i64` and `u63`.
Writes that would grow beyond ReRu's 64 MiB string cap fail. Read-only
bitfield calls do not create a key; failed overflow operations leave it unchanged.
Strings and bitmaps share the same underlying value type.

Sorted-set union/intersection/difference accept sorted-set and plain-set inputs.
`ZUNION` and `ZINTER` support WEIGHTS, SUM/MIN/MAX/COUNT aggregation and
WITHSCORES; store variants replace the destination and clear its TTL.
`ZINTERCARD` supports LIMIT. COUNT aggregation was added to Redis in 8.8;
the local Redis 7.0.15 comparison covers the other algebra options.

`XDEL` and exact `XTRIM MAXLEN|MINID` retain a stream's last generated ID
when entries disappear. The ID survives ReRu snapshots and AOF replay.
Approximate trimming (`~`), `LIMIT`, reference-aware trimming options, and newer
stream features remain unimplemented. New stream snapshots use a revised type
tag; new builds read older tags, but older builds cannot read new snapshots.

## HyperLogLog

`PFADD` accepts binary members and creates an empty sketch when called without
members on a missing key. It returns 1 only when a key is created or a register
changes; duplicate/no-op additions leave the stored bytes and WATCH unchanged.
`PFCOUNT` returns zero for missing keys and estimates a multi-key union without
creating a destination. `PFMERGE` includes the existing destination, supports
aliased or missing source keys, and preserves the destination's TTL. All source
values are validated before replacing the destination.

Sketches are Redis strings with the HYLL header, 16,384 six-bit registers,
MurmurHash64A member hashing and the Ertl improved raw cardinality estimator.
The commands estimate distinct counts; they do not retain members or provide
exact counts, membership queries or element deletion.

New or changed sketches use a 12,304-byte dense value (including the header).
Imported sparse strings are readable and remain unchanged on no-op PFADD;
changes and PFMERGE produce dense storage. Redis's sparse update strategy,
configurable sparse promotion threshold and SIMD optimizations are not
implemented. This trades small-sketch memory efficiency for simpler storage.

Single-key PFCOUNT uses the cached cardinality when valid. Recomputing a dirty
cache updates the string header, invalidates WATCH and persists/replicates that
change, including inside EXEC. It remains a read command usable on replicas
and above maxmemory. Multi-key counts never update source caches. As in Redis,
a valid cache is trusted; arbitrary user edits to HYLL bytes can produce
meaningless counts. Decoding rejects malformed sparse runs and dense register
ranks above 51; the latter is stricter than Redis for manually corrupted values.
Estimates exceeding the RESP signed-integer range saturate at i64::MAX.

Values participate in TTL, transactions, noeviction rollback, snapshots, AOF
rewrite/replay, RDB, DUMP/RESTORE and ReRu replication through string storage.
Tests compare 40,000 binary/text elements, cardinalities, union and merge results,
dense bytes, sparse imports and DUMP exchange against Redis 7.0.15. These are
representative interoperability checks, not a complete numerical proof or a
large-scale performance benchmark.

References: [Redis HyperLogLog](https://redis.io/docs/latest/develop/data-types/probabilistic/hyperloglogs/),
[PFCOUNT](https://redis.io/docs/latest/commands/pfcount/),
[Ertl's estimator](https://arxiv.org/abs/1702.01284).

## Stream consumer groups

`XGROUP CREATE/SETID` supports explicit IDs or `$`, optional `ENTRIESREAD`, and
`MKSTREAM` for creation. CREATECONSUMER/DELCONSUMER manage consumers; deleting a
consumer also removes its pending entries. DESTROY removes the group. Binary
stream, group and consumer names are supported.

`XREADGROUP GROUP group consumer` supports multiple streams, COUNT, BLOCK and
NOACK. `>` distributes new entries; explicit IDs replay that consumer's pending
entries. Deleted or trimmed payloads remain pending and replay as null payloads.
`XACK` removes pending references. `XPENDING` supports summary and range forms,
IDLE, exclusive bounds and a consumer filter. `XCLAIM` supports IDLE, TIME,
RETRYCOUNT, FORCE, JUSTID and LASTID. `XAUTOCLAIM` implements bounded scanning,
COUNT, JUSTID and the Redis 7 deleted-ID reply. These provide at-least-once
processing primitives, not exactly-once application execution.

`XINFO GROUPS/CONSUMERS/STREAM` exposes group cursors, pending counts, consumers,
entry counters and stream endpoints. STREAM FULL accepts COUNT (zero means all)
and returns entries plus group/consumer pending lists with delivery timestamps.
ReRu uses a vector for streams: both radix-tree allocation metrics report zero.
Consumer diagnostics include the Redis 7.2+ inactive/active-time fields. Newer
Redis IDMP and negative-acknowledgement diagnostics are not implemented. Lag is
inferred from counters and retained entries and can be unknown after deletion
or cursor changes; complete parity for every history still needs validation.

`XADD` supports NOMKSTREAM and exact MAXLEN/MINID trimming, with optional `=`.
Appending and trimming are atomic, preserve TTL, and retain pending references.
`XSETID` supports ENTRIESADDED and MAXDELETEDID. Like the pinned Redis 8.10.2
source, it rejects an ID below the existing maximum deleted entry ID; Redis
7.0.15 permits that reset, so this case has a separate target-version test.
Trimming does not advance the maximum explicitly deleted ID.

Group writes participate in transactions, WATCH, memory limits, snapshots, AOF
and ReRu replication. BLOCK is ignored inside EXEC. Competing consumers receive
distinct new entries. Closed blocking clients do not claim subsequent entries;
FIFO waiter order remains pending. Group delivery is a mutation and is rejected
on read-only replicas.
New stream snapshot metadata preserves groups, ownership, retry counts and
consumer/delivery timestamps. Old snapshots load without groups and approximate
entries-added with the retained entry count. New snapshots require matching
builds; old binaries cannot read the new stream type tag.

AOF rebuild recreates groups and pending entries using Redis commands, including
trimmed or deleted payloads and explicitly reset stream IDs. Delivery timestamps
and retry counts survive; consumer seen/active timestamps are recreated during
replay, so idle diagnostics can reset. RDB streams are still unsupported.

Remaining stream work includes reference-aware trimming, XACKDEL/XDELEX/XNACK,
XREADGROUP CLAIM, IDMP, additional options and stream RDB encodings.

Behavior references: [XREADGROUP](https://redis.io/docs/latest/commands/xreadgroup/),
[XINFO STREAM](https://redis.io/docs/latest/commands/xinfo-stream/),
[XSETID](https://redis.io/docs/latest/commands/xsetid/).

## Persistence and replication

- `--dir DIR --dbfilename FILE` loads Redis RDB at startup. `SAVE` writes
  synchronously; `BGSAVE` clones the dataset under the command lock and writes
  from a worker thread. A unique temporary file is synced and renamed over the
  destination only after complete encoding. `LASTSAVE` and `INFO persistence`
  report successful save time, in-progress state and last save status.
- The RDB codec writes version 10 and accepts headers 6 through 15 only for
  supported encodings: plain strings/lists/sets/hashes/binary-score sorted sets,
  integer and LZF strings, intsets, hash/zset/set listpacks, and quicklist2
  lists. CRC64 is verified (zero file checksums are allowed by the format).
  Geo data and bitmaps round-trip through their underlying zsets and strings.
  Expiry is stored as an absolute timestamp; expired keys are omitted on load.
- RDB streams, zipmaps/ziplists/old quicklists, textual-score zsets, field TTL,
  modules, functions, newer specialized types, and nonzero databases are not
  supported. Loading these fails explicitly. Saving a dataset containing a
  stream fails without replacing the existing file. Header acceptance does not
  establish compatibility with every encoding of that Redis version.
- `DUMP`/`RESTORE` transfer the same supported value types. RESTORE supports
  millisecond TTL, `REPLACE` and `ABSTTL`; `IDLETIME`/`FREQ` remain unsupported.
  Restored writes participate in WATCH, transactions, maxmemory, AOF and
  ReRu replication. Invalid payloads leave existing destination data intact.
- RDB files and decoded string budgets are capped at 256 MiB, and key and
  collection counts at one million. There is no save schedule, shutdown save,
  automatic backup recovery, directory fsync, or AOF/RDB combination. SAVE
  and BGSAVE require explicit RDB configuration and are rejected inside MULTI.
  BGSAVE uses an in-memory copy, so large datasets can temporarily double RAM.
- Optional `--snapshot FILE` saves strings, lists, streams, hashes, sets, sorted sets and absolute expiry
  deadlines and restores them at startup. EXPIRE, PERSIST and DEL changes are
  included. This is the custom `RERUSNP1` format, not Redis RDB or AOF.
  New builds can load earlier snapshots. Older builds cannot load snapshots
  containing newer value types; use matching builds for these types.
- Optional `--appendonly FILE` writes the resulting state of changed keys as
  RESP2 `MULTI`/`EXEC` command batches and replays complete batches at startup.
  It preserves binary data, types, and absolute expiry across restarts, and
  truncates an incomplete final batch. Each batch is synced to disk before the
  command response. This is a simple single-file AOF subset; it does not
  implement Redis 7 multipart AOF manifests, automatic rewrite thresholds, configurable
  fsync policy, or arbitrary external AOF import. It cannot be combined with
  `--snapshot` or `--replicaof` yet. Without manual compaction, files grow with writes.
- Optional `--replicaof HOST PORT` provides read-only ReRu replicas, initial
  synchronization, subsequent full dataset snapshots and reconnection.
- Replication uses custom `RERUREP1` frames with changed-key metadata. Use matching
  builds on both ends. Redis PSYNC, partial resynchronization, acknowledgments,
  WAIT and failover are not implemented.
- Persistence errors do not roll back the in-memory command. Disk failure
  recovery and configurable durability policy need further work.

### AOF compaction

`BGREWRITEAOF` requires AOF mode and starts a Rust worker thread. ReRu clones the
dataset and records the current AOF offset under the command gate. The worker
writes a compact RESP transaction to a unique adjacent temporary file. Client
writes continue appending to the original file. At installation, the worker
locks the command gate, copies the exact appended tail, syncs the temporary file,
closes handles, and renames it over the original. Errors before replacement leave
the original AOF intact; ordinary errors clean up the temporary file. Process
termination may leave an unused temporary file, which startup ignores. Unix also
syncs the containing directory; a directory-sync error after rename reports an
error even though replacement has occurred. Windows power-loss durability needs
further validation.

The worker preserves supported types, absolute expiry and stream last-generated
IDs, consumer groups, pending ownership and delivery counts. Only one rewrite
can run at a time. It is rejected inside MULTI and when
AOF is disabled. ReRu does not implement Redis's RDB/rewrite scheduling or
multipart manifest protocol. Dataset cloning temporarily increases memory usage;
the initial clone and final tail copy/fsync block commands. Large workloads still
need performance and crash-fault testing.

`INFO persistence` adds `aof_enabled`, `aof_rewrite_in_progress`,
`aof_last_bgrewrite_status`, `aof_rewrites` (successful rewrites since startup),
`aof_current_size`, and `aof_base_size` (size at startup or last successful rewrite).
`CONFIG GET appendonly` and `CONFIG GET appendfsync` report the startup mode and
fixed `always` fsync policy; changing these through CONFIG SET is unsupported.

Behavior reference: [Redis BGREWRITEAOF](https://redis.io/docs/latest/commands/bgrewriteaof/).

## Client connection lifecycle

`HELLO` and `HELLO 2` return a RESP2 properties array containing ReRu's actual
name/version, protocol 2, stable connection ID, standalone mode, primary/replica
role and an empty modules list. AUTH default password and SETNAME options are
supported. All options are validated before authentication/name changes; failed
credentials preserve an existing authenticated session. `HELLO 3` returns NOPROTO
without changing authentication or the protocol. RESP3 remains unimplemented.

`CLIENT ID` identifies the socket until disconnect, including across RESET.
GETNAME returns nil when unnamed; SETNAME with an empty value clears the name.
Names and library metadata accept printable ASCII without whitespace. SETINFO
supports LIB-NAME and LIB-VER. INFO returns a subset: ID, peer/local addresses,
name, age, database, default user, RESP version and library metadata. CLIENT LIST,
KILL, UNBLOCK, TRACKING, detailed buffer metrics and other client operations
remain pending. Metadata is local to the connection and is never persisted.

`SELECT 0` supports clients that explicitly select the default database. Nonzero
databases fail explicitly; multiple logical databases remain unimplemented.
Connection commands can be queued in MULTI and executed in order without
invalidating dataset watches. They are usable on read-only replicas.

`RESET` executes immediately, including in MULTI, during subscription mode and
before authentication. It discards queued commands/errors, clears WATCH,
removes channel/pattern subscriptions without extra unsubscribe replies, clears
the client name and restores startup authentication requirements. Socket ID and
library metadata remain. On a password-protected server, AUTH or HELLO AUTH is
required again. RESET does not change replica read-only status or dataset data.

Tests cover session isolation, validation, authentication state, transaction
errors, subscription cleanup and replicas. Common connection sequences pass
against Redis 7.0.15. SETINFO is a newer command tested locally against the
specified behavior; its reference-server comparison remains pending.

References: [HELLO](https://redis.io/docs/latest/commands/hello/),
[RESET](https://redis.io/docs/latest/commands/reset/),
[CLIENT SETINFO](https://redis.io/docs/latest/commands/client-setinfo/).

## Authentication

`--requirepass PASSWORD` or `--requirepass-file FILE` configures the default
user's password at startup. An empty password means nopass mode. Files are limited
to 4096 bytes and strip a single trailing LF or CRLF; remaining bytes, including
spaces and binary bytes, are part of the password. The last supplied password
option wins. Prefer file options to avoid credentials appearing in process arguments.

Unauthenticated connections receive NOAUTH for commands other than AUTH, HELLO, RESET and QUIT,
including replication handshakes, subscriptions, configuration and transactions.
`AUTH password` and `AUTH default password` authenticate one connection. Invalid
credentials return WRONGPASS and preserve any existing authenticated session.
With no configured password, named default-user AUTH succeeds, while legacy
password-only AUTH returns Redis's no-password error. AUTH can be queued in an
authenticated transaction and is rejected in RESP2 subscribed mode. `ACL WHOAMI`
returns `default`; no other ACL subcommands or per-user rules are implemented.

`--masterauth PASSWORD` or `--masterauth-file FILE` authenticates a replica to its
primary before the custom REPLSYNC handshake, including reconnects. Replica client
passwords are independent. Wrong upstream credentials prevent synchronization.
Secrets are not included in persistence, CONFIG GET or INFO output.
Comparison uses the `subtle` crate; configured secrets remain in process memory.
There is no TLS, authentication rate limiter, password rotation, ACL persistence,
multi-user support, or runtime CONFIG SET requirepass/masterauth yet.

Behavior references: [AUTH](https://redis.io/docs/latest/commands/auth/) and
[ACL WHOAMI](https://redis.io/docs/latest/commands/acl-whoami/).

## Pub/Sub

Messages are delivered to current subscribers on the same server process.
Pattern subscriptions use byte-wise, case-sensitive `*`, `?`, bracket ranges,
and backslash escapes. A direct and matching pattern subscription on one client
receive separate messages, and `PUBLISH` counts both. Subscribed RESP2 clients
can issue subscription commands, PING and QUIT. Disconnects remove their
subscriptions. Delivery is at-most-once; messages are not persisted or
replicated, and clients on other ReRu servers do not receive them. Writes to a
subscriber socket are synchronous with a 500 ms timeout; slow-client buffering
and backpressure do not yet match Redis. Sharded Pub/Sub, keyspace
notifications, and Pub/Sub ACLs remain pending. RESET clears subscriptions and
returns the connection to ordinary RESP2 command mode.

## Verification for this increment

The local suite has **221 tests**: 22 unit tests in `src/main.rs`, 3 in
`src/lib.rs`, and 196 server integration tests.
Sorted-set tests compare `ZRANGESTORE` output with the matching `ZRANGE` for rank,
score and lex ranges, and cover destination replacement, cleared TTL, deletion on
an empty result, and rejected arity, syntax and wrong types.
It covers binary values, wrong types, score ordering and updates, bitmap ranges,
SET conditions and expiry, multi-key atomicity, WATCH/MULTI/EXEC, snapshot
restart, AOF replay and partial tails, replica updates, and malformed snapshot payloads. Earlier keyspace,
list, stream, hash, set, and transaction regression tests remain. Eviction tests
cover OOM rollback, WATCH, transactions, policy selection, INFO counters,
access order, runtime configuration, and snapshot restart. Keyspace tests cover rename/copy TTL, noeviction rollback,
WATCH, TOUCH/LRU, and blocking-list wakeup. Formatting and Clippy are checked.
Cache lifecycle tests check idle reclamation across multiple batches, rescheduled
and cancelled TTLs, OOM rollback, WATCH invalidation, restart index rebuilding,
AOF/snapshot deletion and replica propagation. Client-limit tests check rejection,
QUIT/disconnect/malformed-request cleanup, atomic configuration validation, and
lowering/raising limits without dropping existing clients.
AOF rewrite tests cover log compaction, all supported types, binary values,
expiry and stream ID preservation, writes/transactions/flush during rewriting,
restart, exact appended-tail copying, failed replacement, command validation,
status fields and retry after an unavailable source file.
Authentication tests cover pre-auth command/replication rejection, per-connection
state, failed reauthentication, transactions, subscriptions, binary password files,
restart, credential exclusion from AOF, and protected primary/replica connections.
The differential suite also checks nopass AUTH and ACL WHOAMI against Redis.

Consumer-group tests cover delivery ownership, NOACK, history, tombstones,
acknowledgements, claiming, blocking/timeouts, concurrent consumers, diagnostics,
TTL, WATCH, OOM rollback, restart/rewrite and replica updates. An additional
reference-dependent test compares group command sequences and XADD options
against Redis 7.0.15; it skips comparisons if the reference port is unset.

HyperLogLog tests add binary members, merge aliasing, malformed input, cached
counts/WATCH, sparse-to-dense OOM rollback, expiry, persistence, replication and
a fourth reference-dependent comparison with Redis. Two unit tests cover packed
register boundaries and malformed sparse runs.

Connection tests cover HELLO option validation, per-socket metadata, password
reset, queued connection commands, WATCH/subscription cleanup and read-only
replicas. A fifth reference-dependent test compares common connection sequences.

The suite includes an optional deterministic RESP2 differential test covering
representative strings, lists, hashes, sets, sorted sets, streams, and transactions.
It passed against a local Redis 7.0.15 instance with
`RERU_REDIS_REFERENCE_PORT=6391 cargo test --test redis_differential`.
Without the environment variable, that test skips its comparisons. The pinned
8.10.2 release and a much broader behavior corpus still need verification.

RDB integration tests cover save/restart, background saving, binary data,
TTL, invalid checksums, failed replacement, WATCH, transactions, OOM rollback,
and AOF restart. With `RERU_REDIS_REFERENCE_PORT` set, the RDB suite exchanges
DUMP payloads with Redis 7.0.15, including compressed and integer strings,
quicklist2, intset, hash listpack and sorted-set listpack encodings. Without
that variable, this reference-dependent test also skips comparisons.
`scripts/test_rdb_interop.py` additionally creates dedicated temporary servers,
exchanges complete RDB files in both directions, and runs `redis-check-rdb`.
Run instructions are in [interoperability testing](INTEROP_TESTING.md).

## Remaining major work

Additional sorted-set and bitmap commands, geo indexing, newer stream features,
sharded Pub/Sub, scripting, broader command options, full Redis RDB/AOF, PSYNC,
multi-user ACLs, TLS, RESP3, cluster/failover, Redis-exact eviction internals, bounded resource usage and performance engineering
remain. Blocking clients do not yet receive Redis's FIFO waiter guarantees.
Redis RDB hash-field expiry and broader hash, list and set command options are
also pending.

Behavior references: [HSET](https://redis.io/docs/latest/commands/hset/),
[HINCRBY](https://redis.io/docs/latest/commands/hincrby/),
[SADD](https://redis.io/docs/latest/commands/sadd/) and
[SMEMBERS](https://redis.io/docs/latest/commands/smembers/).

Follow the [roadmap](ROADMAP.md) and [team plan](ReRu_Implementation_Plan.pdf).
The PDF records the earlier planning baseline; this inventory tracks subsequent
implementation progress.
