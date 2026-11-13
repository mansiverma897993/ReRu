# How ReRu compares

ReRu is one of several servers that speak the Redis protocol. This page says how
it differs from eight of them, so a reader can decide whether to look further.

It is not a benchmark. Throughput and latency are on
[BENCHMARKS.md](BENCHMARKS.md), with the method and the caveats. Everything here
is a design, licensing, or project-status fact, checked on 13 November 2026
against the sources listed below. The recommendation table below is
judgement, not fact.

## At a glance

| | ReRu | Redis | Valkey | Dragonfly | KeyDB | Redka |
| --- | --- | --- | --- | --- | --- | --- |
| Language | Rust | C | C | C++ | C++ | Go |
| Runtime model | One thread per connection; commands serialised by one gate | Single-threaded event loop, optional I/O threads | Single-threaded event loop, optional I/O threads | Multi-threaded, shared-nothing shards | Multi-threaded fork of Redis | In-process or standalone, SQL-backed |
| Protocol | RESP2 only | RESP2 and RESP3 | RESP2 and RESP3 | RESP2 and RESP3 | RESP2 and RESP3 | RESP2 |
| Persistence | Snapshot, single-file AOF, RDB subset | RDB, AOF | RDB, AOF | Snapshots, AOF | RDB, AOF | SQLite or PostgreSQL |
| Cluster / HA | None; ReRu-to-ReRu replication | Cluster, Sentinel | Cluster, Sentinel | Emulated and multi-shard cluster | Cluster, active-active replication | None; relies on the database |
| Scripting | None | Lua, functions | Lua, functions | Lua | Lua | None |
| License | AGPL-3.0-only | 8.x: RSALv2, SSPLv1, or AGPLv3; 7.4: RSALv2 or SSPLv1; 7.2 and earlier: BSD-3 | BSD-3-Clause | Dragonfly Source-Available License, 100 vCPU production limit | BSD-3-Clause | BSD-3-Clause |
| Status | Alpha, experimental | Mature | Mature fork | Production | Stable but dormant; last release 6.3.4, October 2023 | Stable, maintenance mode |

Three more servers that speak the Redis protocol:

| | ReRu | KiviDB | Garnet | Kvrocks |
| --- | --- | --- | --- | --- |
| Language | Rust | Rust | C# (.NET) | C++ |
| Runtime model | One thread per connection; commands serialised by one gate | Multi-threaded, sharded in memory across every core | Multi-threaded; shared-memory network layer on the Tsavorite store | Multi-threaded workers |
| Protocol | RESP2 only | RESP2 and RESP3 | RESP2 and RESP3 | RESP2 and RESP3 |
| Persistence | Snapshot, single-file AOF, RDB subset | Snapshots, optional AOF | Checkpoints, operation log, tiered storage to SSD or cloud | Data lives on disk in RocksDB |
| Cluster / HA | None; ReRu-to-ReRu replication | Replication; cluster with gossip and failover | Cluster with sharding, replication and key migration; failover needs an external control plane | Async replication, Sentinel failover, centrally managed cluster |
| Scripting | None | Lua, functions (in the `lua` and `full` builds) | Lua; C# extensions | Lua (LuaJIT by default) |
| Extras | — | Search, vector, JSON and time-series commands | Vector sets (preview); runs on Linux and Windows | Datasets larger than RAM |
| License | AGPL-3.0-only | AGPL-3.0-only, or a commercial licence; contributors sign a CLA | MIT; contributors sign a CLA | Apache-2.0 |
| Status | Alpha, experimental | Active; v1.2.2, November 2026 | Active; v2.2.11, November 2026 | Apache top-level project; v2.17.0, September 2026 |

## Where ReRu is different

- **Small and readable.** About 12,500 lines of Rust in `src/` (about 21,000
  with tests), one runtime dependency (`subtle`), no async runtime, and
  `#![forbid(unsafe_code)]` in both crates. Every subsystem is a plain module with its own tests.
- **Deliberately serialised.** One `command_gate` mutex runs commands one at a
  time, and each connection owns an OS thread. That keeps the code simple and
  makes ordering easy to reason about. It is also why ReRu falls behind Redis at
  64 connections or with a deep pipeline, as [BENCHMARKS.md](BENCHMARKS.md)
  shows. Dragonfly is multi-threaded by design and was not measured here.
- **Closest neighbour: KiviDB.** It is also Rust and also AGPL-3.0, but it is
  multi-threaded and much further along: RESP3, TLS, ACLs, Lua, cluster, and
  search. If you need a Rust server for real workloads today, look there.
  ReRu differs in scope: a single-gate design, one dependency, a code base
  small enough to read in full, and no CLA.
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
| The same API under a permissive license with foundation governance | Valkey |
| High throughput on a many-core machine | Dragonfly; KeyDB if a dormant upstream is acceptable |
| A Rust server with cluster, search and vector commands | KiviDB |
| A native Windows server, or extensions written in C# | Garnet |
| Datasets larger than memory, on disk | Kvrocks |
| Multi-primary writes | KeyDB, accepting that upstream is dormant |
| Redis-style structures on SQLite or PostgreSQL, or inside a Go process | Redka |
| A small codebase to read, modify, or learn from | ReRu |

ReRu is not a production replacement for Redis. Its own documentation says so in
[README.md](../README.md#project-status) and
[COMPATIBILITY.md](COMPATIBILITY.md).

## Sources

- Redis repository and license: [redis/redis](https://github.com/redis/redis),
  [LICENSE.txt](https://github.com/redis/redis/blob/unstable/LICENSE.txt)
- Valkey: [valkey-io/valkey](https://github.com/valkey-io/valkey),
  [COPYING](https://github.com/valkey-io/valkey/blob/unstable/COPYING),
  [valkey.io](https://valkey.io)
- Dragonfly: [dragonflydb/dragonfly](https://github.com/dragonflydb/dragonfly),
  [LICENSE.md](https://github.com/dragonflydb/dragonfly/blob/main/LICENSE.md)
- KeyDB: [Snapchat/KeyDB](https://github.com/Snapchat/KeyDB),
  [COPYING](https://github.com/Snapchat/KeyDB/blob/main/COPYING)
- Redka: [nalgeon/redka](https://github.com/nalgeon/redka),
  [LICENSE](https://github.com/nalgeon/redka/blob/main/LICENSE)
- KiviDB: [kividbio/kividb](https://github.com/kividbio/kividb),
  [LICENSE](https://github.com/kividbio/kividb/blob/master/LICENSE)
- Garnet: [microsoft/garnet](https://github.com/microsoft/garnet),
  [LICENSE](https://github.com/microsoft/garnet/blob/main/LICENSE),
  [server commands](https://microsoft.github.io/garnet/docs/commands/server)
- Kvrocks: [apache/kvrocks](https://github.com/apache/kvrocks),
  [kvrocks.conf](https://github.com/apache/kvrocks/blob/unstable/kvrocks.conf),
  [LICENSE](https://github.com/apache/kvrocks/blob/unstable/LICENSE)
- ReRu: this repository

Corrections are welcome as a pull request. If a project changes its license or
feature set, update the row and the date above.
