# ReRu 0.1.0-alpha.2

14 November 2026 · Prerelease by Mansi Verma

ReRu is an experimental Redis-compatible server written primarily in safe Rust.
This update expands cache behavior, command discovery, and a bounded Cluster
milestone. It is intended for development and compatibility testing.

## Highlights

- Hash-field expiration: `HEXPIRE`, `HPEXPIRE`, absolute-time variants, TTL
  inspection, `HPERSIST`, `HGETEX`, and `HSETEX`. Field deadlines are removed
  on reads and in the background, and survive ReRu snapshot and AOF reload.
- Static two-node Cluster routing: Redis hash slots and hash tags,
  `CLUSTER KEYSLOT/SHARDS/SLOTS`, `MOVED`, and `CROSSSLOT`. The map is configured
  at startup; it does not discover peers or migrate data.
- `COMMAND`, `COMMAND COUNT`, and `COMMAND INFO` metadata discovery,
  `ZRANGESTORE`, and `CONFIG RESETSTAT`.
- Faster list operations through `VecDeque`, plus corrected blocking move
  timeout replies and several sorted-set range edge cases.
- Expanded project docs, reproducible benchmarks, a RESP parser fuzz target,
  OpenSSF Scorecard, and contributor governance.

The inventory has **222 of 459 Redis 8.10.2 command metadata names (48.4%)**.
This is name presence, not a Redis completion percentage. See the
[compatibility inventory](https://github.com/mansiverma897993/ReRu/blob/v0.1.0-alpha.2/docs/COMPATIBILITY.md)
for supported behavior and known gaps.

## Install and verify

Download the archive for Linux x86_64, macOS arm64/x86_64, or Windows x86_64
from this release. Each archive has a matching `.sha256` file. To build from
source with Rust 1.88 or newer:

```sh
git clone --branch v0.1.0-alpha.2 --depth 1 https://github.com/mansiverma897993/ReRu.git
cd ReRu
cargo test
cargo build --release
./target/release/reru --version
```

On Windows, run `target\release\reru.exe --version`.

The release gate runs formatting, Clippy with warnings denied, 243 Rust tests,
and a release build. CI runs on Linux, macOS, and Windows and checks Rust 1.88
compatibility. A two-node Redis 8.10.2 comparison checked slot vectors,
topology shape, routing errors, and transaction replies.

## Current limits

ReRu remains **experimental**. It speaks RESP2 only. Cluster membership is
static; there is no gossip, slot migration, or failover. Multi-user ACLs, TLS,
Lua scripting, Redis `PSYNC`, and full RDB/AOF compatibility are still pending.
Memory accounting is a logical estimate, and many Redis command options are
not yet supported. Do not use this prerelease as a production Redis replacement.

Licensed under [AGPL-3.0-only](https://github.com/mansiverma897993/ReRu/blob/v0.1.0-alpha.2/LICENSE).
