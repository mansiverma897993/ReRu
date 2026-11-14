# ReRu roadmap

ReRu implements Redis-compatible behavior incrementally. The current
`0.1.0-alpha.2` preview covers strings and cache commands, lists, hashes,
sets, sorted sets, bitmaps, HyperLogLog, geospatial queries, streams with
consumer groups, transactions with `WATCH`, local Pub/Sub, TTLs, ten eviction
policies, and a partial `INFO` surface.

Optional persistence is in place: ReRu snapshots, a single-file AOF with
manual background rewrite, and a subset of Redis RDB. ReRu-to-ReRu
replication and default-user authentication are tested. A static two-node
Cluster slot-routing milestone is available; gossip, migration and failover
remain pending. RESP2 only.

See the [compatibility status](COMPATIBILITY.md) for the command inventory and
the known behavioral gaps.

Redis 8.10.2 is the pinned compatibility reference. Behavior is implemented
from upstream documentation; the project does not translate Redis source.

## Compatibility target

- Use a pinned Redis release for each compatibility test suite so behavior does
  not change during implementation.
- Match RESP replies, errors, binary strings, expiry, and command edge cases.
- Compare observable behavior against a real Redis server and keep regression
  tests for every command before marking it complete.
- Track command-name presence separately from verified behavior and subsystem
  coverage. The current 222/459 metadata entries (48.4%) do not establish
  30–40% implementation of Redis.
- Keep the server usable after every small, tested commit.

## Work remaining

1. Complete remaining list options and newer stream features: reference-aware
   trimming/deletion, IDMP, negative acknowledgements and CLAIM reads. Add FIFO
   waiter ordering and disconnect cancellation for blocking commands.
2. Expand sorted sets, bitmaps, and geospatial indexing; extend hashes and
   sets with remaining command options and Redis RDB field-TTL support. Existing scans, random
   access, moves and set algebra still need broader compatibility tests.
3. Expand transaction compatibility testing, Pub/Sub coverage, scripting, and
   the remaining keyspace commands.
4. Expand RDB to streams, legacy encodings and modern Redis types; add save
   scheduling and broader crash recovery. Add automatic AOF rewrite, configuration,
   multipart manifests, and interoperability.
5. Add Redis PSYNC replication, failover, clustering, multi-user ACLs, TLS, and
   configuration.
6. Bring maxmemory accounting and eviction internals closer to Redis, then add RESP3, performance and resource controls, operational tooling, and
   compatibility testing at scale.

## Current limits

- By default data is held only in memory. `--snapshot FILE` saves after each
  write using a ReRu-specific format; it is not Redis RDB. `--appendonly FILE`
  offers single-file RESP command batches with fsync per batch; it lacks Redis
  multipart AOF and automatic rewrite scheduling and cannot be combined with snapshots or replicas.
  Manual BGREWRITEAOF compacts the file, but cloning and final installation still
  block commands and need performance work.
- Explicit `--dir`/`--dbfilename` enables Redis RDB startup loading and manual
  SAVE/BGSAVE. The supported subset exchanges files with Redis 7.0.15;
  stream/legacy/new specialized encodings and automatic saves remain pending.
- Replicas use full snapshot transfer on each write and resynchronize after a
  disconnect. They do not support Redis PSYNC, offsets, or backlog recovery.
  Custom replication frames carry changed-key metadata for replica `WATCH`;
  a full resynchronization invalidates every active watch on that replica.
- Only RESP array requests and RESP2 replies are supported.
- Each client uses one operating-system thread, bounded by `--maxclients`
  (default 10000). The optional maxmemory ceiling uses estimated logical dataset size,
  not process memory. Bitmap values have a 64 MiB growth cap.
- Ten maxmemory policies are available. ReRu's memory accounting, LRU/LFU
  algorithms, INFO output, and most CONFIG parameters are not Redis exact.
- Blocking list operations wake waiting clients, but Redis's FIFO waiter
  ordering is not implemented yet.
- This project is not a production Redis replacement at this stage.

Upstream references: <https://github.com/redis/redis> and
<https://redis.io/docs/latest/commands/>. Redis 7.2 and earlier use BSD-3-Clause;
Redis 8 offers RSALv2, SSPLv1, or AGPLv3. Any direct source translation must
retain the license and attribution required by the selected release.
