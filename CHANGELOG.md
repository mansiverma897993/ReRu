# Changelog

All notable changes to ReRu are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
Because ReRu is pre-1.0, minor versions may include breaking changes.

## [Unreleased]

### Added

- `--bind ADDRESS` startup option so the server can listen on a container
  network while still defaulting to `127.0.0.1`.
- `Dockerfile` and `.dockerignore` for container builds.
- Open-source project scaffolding: MIT license, contributing guide, code of
  conduct, security policy, changelog, architecture and interoperability docs,
  issue and pull request templates, and CI.
- README banner, at-a-glance statistics, feature cards, architecture diagram,
  and social preview assets.

### Fixed

- A transient socket accept error no longer shuts down the whole server. The
  listener logs the error and continues accepting connections.
- Integration tests no longer race for ports or temporary file names. Each
  server binds an operating-system-assigned port and reports it on startup, and
  temporary paths include a per-process sequence number.
- Replaced the deprecated `AtomicUsize::fetch_update` with a compare-exchange
  loop, so the client-limit check builds cleanly on both the MSRV and the
  newest stable toolchain.
- The pub/sub disconnect test allows a bounded window for the listener thread
  to release its subscriptions, removing a Windows-only flake.

## [0.1.0-alpha.1] - 2026-10-28

First public preview. Experimental; not a production Redis replacement.

### Added

- RESP2 server with binary-safe strings, lists, hashes, sets, sorted sets,
  bitmaps, streams with consumer groups, and geospatial commands.
- Transactions (`MULTI`, `EXEC`, `DISCARD`, `WATCH`, `UNWATCH`) and local
  pub/sub.
- TTLs, deadline-indexed background expiry, ten eviction policies, configurable
  memory estimates, and client connection limits.
- Persistence: ReRu snapshots, single-file AOF with manual `BGREWRITEAOF`
  compaction, and a subset of Redis RDB including `SAVE`, `BGSAVE`, `DUMP`, and
  `RESTORE`.
- ReRu-to-ReRu replication with authenticated upstream connections.
- Default-user authentication via `AUTH`, `ACL WHOAMI`, `--requirepass`, and
  `--requirepass-file`.
- HyperLogLog `PFADD`, `PFCOUNT`, and `PFMERGE` with Redis dense/sparse binary
  interchange.
- Stream consumer groups, pending-message recovery, `XINFO` diagnostics, and
  atomic `XADD` trimming.
- RESP2 `HELLO`, `SELECT 0`, `CLIENT` metadata commands, and `RESET`.
- Partial `INFO` sections for memory, stats, clients, and persistence.
- 204 integration and unit tests, including differential tests against Redis
  7.0.15.
- Windows packaging script and Redis RDB/AOF interoperability helpers.

### Known limitations

- RESP2 only; no RESP3.
- No TLS and no multi-user ACLs.
- No Lua, functions, clustering, failover, or Redis `PSYNC`.
- Command coverage is 203 of 459 Redis 8.10.2 metadata entries (44.2% name
  presence), not complete behavior.
- Blocking commands do not yet implement Redis FIFO waiter ordering.
- Memory limits use a logical dataset estimate, not process RSS.

[Unreleased]: https://github.com/mansiverma897993/ReRu/compare/v0.1.0-alpha.1...HEAD
[0.1.0-alpha.1]: https://github.com/mansiverma897993/ReRu/releases/tag/v0.1.0-alpha.1
