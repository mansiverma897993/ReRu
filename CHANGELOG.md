# Changelog

All notable changes to ReRu are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
Because ReRu is pre-1.0, minor versions may include breaking changes.

## [Unreleased]

### Added

- Added bounded Q8/FP32 vector sets with `VADD`, `VCARD`, `VDIM`, `VEMB`,
  `VREM`, and exact `VSIM ... TRUTH`, including key expiry, transactions,
  maxmemory rollback, ReRu snapshot/AOF persistence and replication.

### Fixed

- `MULTI` now uses `COMMAND` arity metadata for ordinary commands: variadic
  commands queue at their minimum arity and report invalid argument shapes in
  `EXEC`, while fixed-arity errors still abort at queue time.
- A Pub/Sub subscriber removed after a failed publish no longer aborts the
  server when it sends another subscription command.
- Large replies are retried until the client drains them instead of being
  truncated by the 500 ms socket write timeout.
- The AOF writer splits large keys below the replay bulk limit, and snapshots
  with more than 1,000,000 keys decode again.
- AOF write failures return `-MISCONF` and keep the client connection open.
- `ZADD ... GT|LT INCR` compares the resulting score instead of the increment.
- Negative hash-field expiry values are rejected instead of deleting fields.
- Out-of-range score and weight literals are rejected instead of storing
  infinity.
- Arity errors on `EXEC`, `DISCARD`, `MULTI`, and `WATCH` abort the queued
  transaction. Variadic `PING`, `FLUSHDB`, and `FLUSHALL` queue and report
  their argument error inside `EXEC`; `INFO` accepts multiple sections and the
  `all` alias, and `FLUSHDB`/`FLUSHALL` reject excess arguments with
  `ERR syntax error`. Replicas now skip eviction when `maxmemory` is lowered.
- `CONFIG SET` is accepted on a replica, and `COPY` with identical keys returns
  the Redis error.
- `GETRANGE`, `SUBSTR`, `BITCOUNT`, and `BITPOS` clamp very negative end indexes
  like Redis, and `BITPOS` distinguishes a missing key from an empty string.
- `GEOPOS` and `WITHCOORD` print coordinates in Redis's shortest round-trip and
  small-value exponent form, and fractional `STOREDIST` destination scores are
  readable by GEO commands again.
- Subscription acknowledgements share a 750 ms batch budget and publish
  deliveries a 100 ms budget (100 ms and 20 ms without progress), so a
  non-reading subscriber cannot hold the command gate for tens of seconds.
- `XAUTOCLAIM` counts tombstone deletions against `COUNT`, so the claimed
  entries and the returned cursor match Redis.
- Blocked `BLPOP`, `BLMOVE`, `BLMPOP`, and `XREAD` clients no longer wake with
  WRONGTYPE on a type change alone; multi-key forms still report it once another
  key can serve data.
- The parser accepts up to 1,048,576 arguments and 256 MiB of argument bytes,
  below Redis's defaults so one command fits ReRu's persistence frames.
- Malformed frames reply `-ERR Protocol error: <reason>` before closing, and
  empty `*0`/`*-1` frames are ignored like Redis.
- `HELLO` reports `replica` on a read-only replica, matching Redis's HELLO role
  field. `INFO` still has no replication section.
- `PUBSUB NUMPAT` counts unique patterns instead of each client's subscription.
- `PFADD`, `PFCOUNT`, and `PFMERGE` use the generic WRONGTYPE error for non-string
  keys; a string with a bad sketch header keeps the HyperLogLog-specific text.
- `QUIT` ignores extra arguments, replies `+OK`, and closes the connection.
- `HSETEX`, `HGETEX`, `HEXPIRE`, and `HPEXPIRE` accept condition and expiry
  options before or after `FIELDS`, report `unknown argument: <token>` for
  unrecognized tokens, and use Redis's texts for duplicate `FIELDS`, conflicting
  flags, short or extra field lists, and invalid counts. The `HTTL`/`HPERSIST`
  family uses Redis's own field-count messages. `HGETEX` is always treated as a
  write, so options placed after `FIELDS` are persisted and rejected on a
  replica, and a missing expiry value reports `missing expire time`.
- `GETEX` validates option syntax first, then the key and type, and only then
  the expiry value, so a missing key replies nil and a wrong type replies
  WRONGTYPE without value errors.
- `XCLAIM` reports `Invalid IDLE/TIME/RETRYCOUNT option argument for XCLAIM`
  for unusable option values, accepts negative values, and `XSETID ...
  ENTRIESADDED` rejects negatives with `entries_added must be positive`.
- `XAUTOCLAIM` and `XCLAIM` accept a negative `min-idle-time` (every pending
  entry is eligible) and reject non-canonical values with Redis's
  `Invalid min-idle-time argument` text. `XAUTOCLAIM COUNT` rejects zero,
  negative, non-canonical, and overflow values with `COUNT must be > 0`.
- `SET` and `GETEX` parse only the last value of a repeated expiry option and
  reject an `NX`/`XX` conflict as soon as the second flag is seen, so a bad
  earlier value or flag pair no longer wins the error.
- Integer command arguments now use Redis's canonical `string2ll` form across
  `INCRBY`/`DECRBY`/`INCR`, list and sorted-set indexes and counts, hash and
  key expiry, bitmap offsets and widths, stream trim limits, `SELECT`, `HELLO`,
  scan `COUNT`, multi-key `numkeys`/`COUNT`, geo `COUNT`, and stream-group
  options; values such as `010`, `+1` and `-0` are rejected. Wire-level bulk and
  multibulk lengths are canonical as well. Scan cursors and stream IDs keep
  Redis's lenient parsers.
- `SUBSCRIBE`, `UNSUBSCRIBE`, `PSUBSCRIBE`, and `PUNSUBSCRIBE` inside `MULTI`
  are queued and run by `EXEC`; the connection enters subscribed mode with the
  acknowledgements nested in the `EXEC` reply. A `PING` after a subscription in
  the same `EXEC` uses the subscribed reply shape, messages published to the
  connection inside `EXEC` are delivered after the transaction reply, and
  `EXEC` inside subscribed mode replies `EXECABORT`. The subscribed-mode
  rejection message now uses Redis's text.
- Sorted-set score replies use Redis-style formatting: exact integers print
  without an exponent and small or very large magnitudes use exponent form
  (`1e+300`, `1e-7`, `1e+20`) instead of a fully expanded decimal. Digit
  selection uses Rust's shortest round-trip algorithm, so some values differ
  from Redis's Grisu2 in trailing digits, and values between about `1e18` and
  `1e23` can print in exponent form where Redis keeps a fixed decimal.
- `SPOP`, `SRANDMEMBER`, and `HRANDFIELD` return the whole collection for a
  positive count above its size instead of a count error, and validate the count
  before the key lookup. `SRANDMEMBER`/`HRANDFIELD` reject `i64::MIN` with
  Redis's range error; `HRANDFIELD WITHVALUES` rejects counts outside
  ±`i64::MAX/2`; `SPOP` rejects negative or non-canonical counts with
  `value is out of range, must be positive`.
- `XRANGE`/`XREVRANGE` return a null array for `COUNT 0` or a negative count on
  an existing stream, and an empty array for a missing key. Bounds are validated
  before the count, non-canonical counts are rejected, and counts above `i64`
  range now error instead of returning every entry.
- `XCLAIM` and `XAUTOCLAIM` register their consumer even when no entry is
  claimed, matching Redis's `XINFO CONSUMERS`.
- `XREAD` and `XREADGROUP` treat `COUNT 0` or a negative count as unlimited.
- `XACK` and `XDEL` resolve the key, type, and group before parsing IDs, so a
  missing key replies 0 and a wrong type replies WRONGTYPE.
- `SET` and `GETEX` accept repeated flags and repeated expiry options with the
  same unit (last wins); mixing units or combining KEEPTTL/PERSIST with an
  expiry is a syntax error.
- `SET`, `SETEX`, `PSETEX`, and `GETEX` report `value is not an integer or out of
  range` for non-canonical or out-of-range integers, and reject zero, negative,
  or second-to-millisecond overflow with `invalid expire time`.

## [0.1.0-alpha.2] - 2026-11-15

### Added

- Command latency monitoring with the `latency-monitor-threshold` runtime
  setting and `LATENCY LATEST`, `HISTORY`, and `RESET`. Monitoring is disabled
  by default, blocking wait time is excluded, same-second spikes merge by
  maximum latency, and at most 160 recent command samples are kept.
- `OBJECT ENCODING` and `OBJECT REFCOUNT`. The encoding is an advisory Redis 8
  label for the stored value, and every present key reports one reference.
- `LCS` with `LEN`, `IDX`, `MINMATCHLEN`, and `WITHMATCHLEN`. All forms are
  capped at 8,000,000 table cells.
- Static two-node Cluster routing with CRC16/hash-tag slots, `CLUSTER KEYSLOT`,
  `CLUSTER SHARDS`, `CLUSTER SLOTS`, `MOVED`, and `CROSSSLOT` replies. Dynamic
  membership, migration, and failover remain unsupported.
- Hash-field TTL commands (`HEXPIRE`, `HPEXPIRE`, `HEXPIREAT`, `HPEXPIREAT`,
  `HTTL`, `HPTTL`, `HEXPIRETIME`, `HPEXPIRETIME`, `HPERSIST`, `HGETEX`, `HSETEX`),
  with background expiry and ReRu snapshot/AOF preservation.
- OpenSSF Scorecard workflow (`.github/workflows/scorecard.yml`), run weekly and
  on pushes to `main`, with third-party actions pinned to commit SHAs.
- "Claiming an issue" rules in `CONTRIBUTING.md` for program contributors
  (SWoC, GSSoC, 24 Pull Requests).
- `docs/COMPARISON.md`: a sourced table of how ReRu differs from Redis, Valkey,
  Dragonfly, KeyDB, Redka, KiviDB, Garnet and Kvrocks, including where ReRu is
  weaker.
- `GOVERNANCE.md`: the owners, the lazy-consensus decision rule, the no-CLA
  contribution terms, and the release and response-time commitments.
- SPDX license headers (`// SPDX-License-Identifier: AGPL-3.0-only`) added to all files under `src/` and `tests/`.
- Docker Compose quickstart with AOF persistence (`docker-compose.yml`),
  documented in the README.
- Contributor recognition: `.all-contributorsrc` and `CONTRIBUTORS.md` seeded from existing commit authors, linked from the README and CONTRIBUTING.md.
- CLI reference: every startup flag documented in [docs/CLI.md](docs/CLI.md),
  linked from the README and the architecture notes.
- `ZRANGESTORE`, which stores a rank, score or lex range from one sorted set into
  another. The destination is replaced, loses any TTL, and is deleted when the
  range is empty. `LIMIT` without `BYSCORE` or `BYLEX` now returns the Redis
  error text.
- `scripts/bench_vs_redis.py` and `docs/BENCHMARKS.md`: a reproducible
  comparison against a running Redis with medians, p99 latency, and an explicit
  list of what the numbers do not prove.
- `cargo-fuzz` target `resp_parser` that drives the RESP command parser with
  arbitrary bytes, plus a committed seed corpus. The crate now also builds as a
  library so the parser is reachable from `fuzz/`. Fuzzing needs a nightly
  toolchain and is run by hand; see `docs/INTEROP_TESTING.md`.
- Redis-compatible `COMMAND`, `COMMAND COUNT`, and `COMMAND INFO` metadata
  discovery, including nil entries for unknown command names.
- `CONFIG RESETSTAT` to clear the statistics counters exposed by `INFO`.

### Changed

- README banner, architecture diagram, and social preview redrawn with light and
  dark variants, generated by `scripts/gen_readme_assets.py`. The README header
  adds live CI and release badges.
- Both crates declare `#![forbid(unsafe_code)]`. Adding an `unsafe` block now
  fails the build instead of relying on review.

### Fixed

- `BLMOVE` and `BRPOPLPUSH` return a nil array when they time out, as Redis does,
  while keeping a nil bulk inside `MULTI` where they do not block. `LPOP`, `RPOP`
  and the `LPOS` options now use Redis's error text for unusable counts.
- Lists use a `VecDeque`, so `LPUSH` and `LPOP` no longer copy the whole list. At
  20,000 elements they are about 2.5x faster and p99 falls from about 12 ms to
  about 1.6 ms under load.
- Sorted-set ranges now match Redis for a negative `LIMIT` offset (an empty
  selection instead of an error), for unparsable score bounds
  (`ERR min or max is not a float`), and for an empty score bound (read as zero).
- Closed blocking list, sorted-set and stream clients stop waiting promptly and
  cannot consume later writes; client slots are released after disconnect.

## [0.1.0-alpha.1] - 2026-11-07

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
- `--bind ADDRESS` startup option so the server can listen on a container
  network while still defaulting to `127.0.0.1`.
- `Dockerfile` and `.dockerignore` for container builds.
- Open-source project scaffolding: AGPL-3.0 license, contributing guide, code of
  conduct, security policy, changelog, architecture and interoperability docs,
  issue and pull request templates, and CI.
- README banner, at-a-glance statistics, feature cards, architecture diagram,
  and social preview assets.
- One-command development setup: `Makefile`, `cargo` aliases, a pinned toolchain,
  a dev container, VS Code recommendations, and `docs/DEVELOPMENT.md` with
  troubleshooting.
- 204 integration and unit tests, including differential tests against Redis
  7.0.15.
- Windows packaging script and Redis RDB/AOF interoperability helpers.
- 20 scoped contributor issues with area, difficulty, and good-first labels.

### Changed

- License changed from MIT to AGPL-3.0 before the first public release, so
  modified versions and hosted forks must publish their source.

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

### Known limitations

- RESP2 only; no RESP3.
- No TLS and no multi-user ACLs.
- No Lua, functions, clustering, failover, or Redis `PSYNC`.
- Command coverage is 203 of 459 Redis 8.10.2 metadata entries (44.2% name
  presence), not complete behavior.
- Blocking commands do not yet implement Redis FIFO waiter ordering.
- Memory limits use a logical dataset estimate, not process RSS.

[Unreleased]: https://github.com/mansiverma897993/ReRu/compare/v0.1.0-alpha.2...HEAD
[0.1.0-alpha.2]: https://github.com/mansiverma897993/ReRu/releases/tag/v0.1.0-alpha.2
[0.1.0-alpha.1]: https://github.com/mansiverma897993/ReRu/releases/tag/v0.1.0-alpha.1
