# How ReRu compares

ReRu is one of several servers that speak the Redis protocol. This page says how
it differs from five of them, so a reader can decide whether to look further.

It is not a benchmark. Throughput and latency are on
[BENCHMARKS.md](BENCHMARKS.md), with the method and the caveats. Everything here
is a design or licensing fact, checked on 13 November 2026 against the sources
listed at the end.

## At a glance

| | ReRu | Redis | Valkey | Dragonfly | KeyDB | Redka |
| --- | --- | --- | --- | --- | --- | --- |
| Language | Rust | C | C | C++ | C++ | Go |
| Runtime model | One thread per connection; commands serialised by one gate | Single-threaded event loop, optional I/O threads | Single-threaded event loop, optional I/O threads | Multi-threaded, shared-nothing shards | Multi-threaded fork of Redis | In-process or standalone, SQL-backed |
| Protocol | RESP2 only | RESP2 and RESP3 | RESP2 and RESP3 | RESP (Redis clients) | RESP (Redis 5 era) | RESP (core commands) |
| Persistence | Snapshot, single-file AOF, RDB subset | RDB, AOF | RDB, AOF | Snapshots, AOF | RDB, AOF | SQLite or PostgreSQL |
| Cluster / HA | None; ReRu-to-ReRu replication | Cluster, Sentinel | Cluster, Sentinel | Emulated cluster | Active-active replication | None; relies on the database |
| Scripting | None | Lua, functions | Lua, functions | Lua | Lua | None |
| Licence | AGPL-3.0-only | RSALv2, SSPLv1, or AGPLv3; BSD-3 for 7.2 and earlier | BSD-3-Clause | Dragonfly Source-Available Licence, 100 vCPU production limit | BSD-3-Clause | BSD-3-Clause |
| Status | Alpha, experimental | Mature | Mature fork | Production | Production | Stable, maintenance mode |

## Where ReRu is different

- **Small and readable.** About 12,500 lines of Rust in `src/`, one runtime
  dependency (`subtle`), no async runtime, and `#![forbid(unsafe_code)]` in both
  crates. Every subsystem is a plain module with its own tests.
- **Deliberately serialised.** One `command_gate` mutex runs commands one at a
  time, and each connection owns an OS thread. That keeps the code simple and
  makes ordering easy to reason about. It is also why ReRu loses to Redis and
  Dragonfly under concurrency, as the benchmark page shows.
- **Fewer moving parts, fewer features.** No RESP3, TLS, multi-user ACLs,
  scripting, cluster, Sentinel, PSYNC, or Redis-exact RDB/AOF. What exists is
  listed in [COMPATIBILITY.md](COMPATIBILITY.md).
- **Clean-room.** The implementation follows the Redis documentation and observed
  behaviour. No Redis source is copied or translated; see
  [CONTRIBUTING.md](../CONTRIBUTING.md).
- **AGPL-3.0-only.** A hosted fork must publish its source. Redis 8 offers AGPLv3
  as one of three options; Valkey, KeyDB and Redka use BSD-3-Clause, which has no
  such condition.

## Which one to use

| If you want | Look at |
| --- | --- |
| A drop-in server with the full command set and ecosystem | Redis or Valkey |
| The same API under a permissive licence with foundation governance | Valkey |
| High throughput on a many-core machine | Dragonfly or KeyDB |
| Multi-primary writes | KeyDB |
| Redis-style structures on SQLite or PostgreSQL, or inside a Go process | Redka |
| A small codebase to read, modify, or learn from | ReRu |

ReRu is not a production replacement for Redis. Its own documentation says so in
[README.md](../README.md#project-status) and
[COMPATIBILITY.md](COMPATIBILITY.md).

## Sources

- Redis repository and licence: [redis/redis](https://github.com/redis/redis),
  [LICENSE.txt](https://github.com/redis/redis/blob/unstable/LICENSE.txt)
- Valkey: [valkey-io/valkey](https://github.com/valkey-io/valkey),
  [valkey.io](https://valkey.io)
- Dragonfly: [dragonflydb/dragonfly](https://github.com/dragonflydb/dragonfly),
  [LICENSE.md](https://github.com/dragonflydb/dragonfly/blob/main/LICENSE.md)
- KeyDB: [Snapchat/KeyDB](https://github.com/Snapchat/KeyDB)
- Redka: [nalgeon/redka](https://github.com/nalgeon/redka)
- ReRu: this repository

Corrections are welcome as a pull request. If a project changes its licence or
feature set, update the row and the date at the top.
