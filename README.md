<div align="center">

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/assets/banner-dark.svg">
  <img src="docs/assets/banner-light.svg" alt="ReRu: a Redis-compatible data server written in Rust" width="100%">
</picture>

<br><br>

[![CI](https://img.shields.io/github/actions/workflow/status/mansiverma897993/ReRu/ci.yml?branch=main&style=flat-square&label=CI&labelColor=1A1714)](https://github.com/mansiverma897993/ReRu/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/mansiverma897993/ReRu?include_prereleases&style=flat-square&label=release&labelColor=1A1714&color=C2410C)](https://github.com/mansiverma897993/ReRu/releases)
[![License](https://img.shields.io/badge/license-AGPL--3.0--only-C2410C?style=flat-square&labelColor=1A1714)](LICENSE)
[![MSRV](https://img.shields.io/badge/rust-1.88%2B-6B645A?style=flat-square&logo=rust&logoColor=white&labelColor=1A1714)](https://www.rust-lang.org)
[![unsafe](https://img.shields.io/badge/unsafe-forbidden-6B645A?style=flat-square&labelColor=1A1714)](https://github.com/rust-secure-code/safety-dance)
[![Dependencies](https://img.shields.io/badge/runtime%20deps-1-6B645A?style=flat-square&labelColor=1A1714)](Cargo.toml)
[![Redis reference](https://img.shields.io/badge/reference-Redis%208.10.2-6B645A?style=flat-square&labelColor=1A1714)](docs/COMPATIBILITY.md)
[![Platforms](https://img.shields.io/badge/platforms-linux%20%7C%20macos%20%7C%20windows-6B645A?style=flat-square&labelColor=1A1714)](#quick-start)

**ReRu speaks RESP2, so existing Redis clients connect without changes.**<br>
It reimplements a growing slice of Redis and its persistence formats in safe, dependency-light Rust.

[Quick start](#quick-start) ·
[Compatibility](docs/COMPATIBILITY.md) ·
[Architecture](docs/ARCHITECTURE.md) ·
[Roadmap](docs/ROADMAP.md) ·
[Contributing](CONTRIBUTING.md)

</div>

> [!WARNING]
> ReRu is experimental. Use it for development and compatibility experiments,
> not production. See [Project status](#project-status).

## Why ReRu

- **Familiar protocol.** Binary-safe RESP2 over TCP. Works with `redis-cli`,
  `redis-py`, `ioredis`, Lettuce, and any client that uses the supported
  commands.
- **Small and readable.** About 21k lines of straight-line Rust with one
  runtime dependency and no `unsafe`, enforced by `#![forbid(unsafe_code)]`.
  Every subsystem is a plain module with its own tests.
- **Research-friendly.** Each command follows the pinned Redis 8.10.2
  reference, with differential tests against a live Redis where possible.

Compared with Redis, Valkey, Dragonfly, KeyDB, Redka, KiviDB, Garnet and Kvrocks:
[docs/COMPARISON.md](docs/COMPARISON.md).

## Quick start

Install a Rust toolchain (1.88 or newer), then build and run:

```sh
git clone https://github.com/mansiverma897993/ReRu.git
cd ReRu
cargo build --release

./target/release/reru \
  --port 6379 \
  --maxmemory 64mb \
  --maxmemory-policy allkeys-lru \
  --maxclients 256
```

The server listens on `127.0.0.1` by default and prints its startup line to
stderr. The binary is `target/release/reru` on Unix and `target\release\reru.exe`
on Windows. Run `reru --help` for all flags.

## Connect a client

Use any Redis client pointed at `127.0.0.1:6379`:

```sh
redis-cli SET greeting "Hello from ReRu" EX 60
redis-cli GET greeting
redis-cli TTL greeting
redis-cli RPUSH queue a b c
redis-cli LRANGE queue 0 -1
```

In Python:

```python
import redis

client = redis.Redis(host="127.0.0.1", port=6379)
client.set("greeting", "Hello from ReRu", ex=60)
print(client.get("greeting"))
```

## Features

| Area | What works |
| --- | --- |
| Data structures | Strings, lists, hashes, sets, sorted sets, bitmaps, streams with consumer groups, geospatial queries |
| Cache behavior | Conditional writes, TTLs, background expiry, ten eviction policies, configurable memory estimates |
| Transactions and messaging | `MULTI`, `EXEC`, `WATCH`, blocking reads, local Pub/Sub, stream acknowledgements, pending-message recovery |
| Persistence | ReRu snapshots, single-file AOF with online rewrite, a tested subset of Redis RDB |
| Replication | Read-only ReRu replicas over an authenticated snapshot-plus-stream protocol |
| Auth and operations | Default-user `AUTH`, client limits, runtime memory configuration, partial `INFO` metrics |

**204 of 459 command metadata entries** from Redis 8.10.2 have implementation
paths: **44.4% command-name coverage**. This counts command names, not complete
behavior. Supported options, differences, and tests are listed in the
[compatibility inventory](docs/COMPATIBILITY.md).

## Architecture

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/assets/architecture-dark.svg">
  <img src="docs/assets/architecture-light.svg" alt="ReRu request path: clients, RESP2 parser, command dispatch, shared store, then snapshots, AOF, RDB, and replicas" width="100%">
</picture>

One connection thread per client. Writes are serialized by the command gate and
fan out from a single write-out point. Read the full
[architecture notes](docs/ARCHITECTURE.md) for the module map and the command
path.

## Server options

Defaults, valid values, valid combinations, and error text for every flag are in
the [CLI reference](docs/CLI.md).

| Option | Default | Description |
| --- | --- | --- |
| `--port PORT` | `6379` | TCP port to listen on |
| `--bind ADDRESS` | `127.0.0.1` | IP address to bind. Use `0.0.0.0` only inside a trusted network (for example a container) |
| `--maxclients COUNT` | `10000` | Maximum concurrent client connections |
| `--maxmemory SIZE` | `0` (unlimited) | Logical dataset memory limit, e.g. `64mb`, `1gb` |
| `--maxmemory-policy POLICY` | `noeviction` | Eviction policy |
| `--snapshot FILE` | off | Save and restore a ReRu snapshot |
| `--appendonly FILE` | off | Single-file AOF with replay and manual rewrite |
| `--dir DIR` | — | Directory for RDB files |
| `--dbfilename FILE` | `dump.rdb` | RDB filename |
| `--replicaof HOST PORT` | off | Run as a read-only replica |
| `--requirepass PASSWORD` | off | Require a default-user password |
| `--requirepass-file FILE` | off | Read the client password from a file |
| `--masterauth PASSWORD` | off | Password for the upstream primary |
| `--masterauth-file FILE` | off | Read the upstream password from a file |
| `--version`, `--help` | — | Print version or usage |

Eviction policies: `noeviction`, `allkeys-lru`, `allkeys-lrm`, `allkeys-lfu`,
`allkeys-random`, `volatile-lru`, `volatile-lrm`, `volatile-lfu`,
`volatile-random`, `volatile-ttl`.

## Persistence

Data stays in memory unless a persistence mode is selected. Choose one mode:

| Startup options | Behavior |
| --- | --- |
| `--snapshot ./reru.snap` | Save after writes and restore at startup; ReRu-specific format |
| `--appendonly ./reru.aof` | Append RESP transactions, fsync each batch, replay at startup |
| `--dir . --dbfilename dump.rdb` | Load supported Redis RDB data; save with `SAVE` or `BGSAVE` |

With AOF enabled, `redis-cli BGREWRITEAOF` compacts write history in a
background worker while preserving later writes. Check `INFO persistence` for
progress. Rewriting is manual; Redis multipart AOF and automatic rewrite
thresholds are not implemented.

`--replicaof 127.0.0.1 6379 --port 6380` starts a replica. Both servers must run
matching ReRu builds; replication is not Redis `PSYNC`. AOF mode cannot be
combined with replica mode.

## Authentication

Use `--requirepass-file ./password.txt` to require the default user's password,
then connect with `redis-cli --askpass`. Password files accept up to 4096 bytes
and strip one trailing line ending. Replicas use
`--masterauth-file ./primary-password.txt` to authenticate upstream; their own
client password is configured separately. `--requirepass` and `--masterauth`
also accept passwords directly.

Authentication does not encrypt traffic. TLS and multi-user ACL policies are
still pending.

## Docker

```sh
docker build -t reru .
docker run --rm -p 6379:6379 reru
```

The image binds to `0.0.0.0` inside the container so the port mapping works.
Keep that exposure inside a trusted network and set a password for anything
beyond local development.

### Docker Compose (with persistence)

For a one-command setup with AOF persistence across restarts:

```sh
docker compose up -d --build
```

This starts three services:

- `init` — a one-off container that creates `/home/reru/data` on a named
  volume and sets its ownership to the `reru` user (uid `10001`), since the
  image runs as non-root.
- `reru` — the server, built from the local `Dockerfile`, with
  `--appendonly /home/reru/data/reru.aof` enabled so data survives restarts.
- `client` — the official `redis:7` image, idle, for testing with
  `redis-cli` against the `reru` service.

Try it:

```sh
docker compose exec client redis-cli -h reru SET greeting "Hello from ReRu"
docker compose exec client redis-cli -h reru GET greeting
docker compose restart reru
docker compose exec client redis-cli -h reru GET greeting   # still there
```

Stop everything:

```sh
docker compose down       # keeps the data volume
docker compose down -v    # also deletes the data volume
```

## Development

<details>
<summary><b>Commands, layout, and test setup</b></summary>

<br>

[![Open in GitHub Codespaces](https://github.com/codespaces/badge.svg)](https://codespaces.new/mansiverma897993/ReRu)

```sh
git clone https://github.com/mansiverma897993/ReRu.git
cd ReRu
make setup     # or: cargo fetch
make test      # 221 tests
make run       # or: cargo run -- --port 6379
make check     # fmt + clippy + tests
```

`make` is optional. The `cargo` aliases in `.cargo/config.toml` work on every
platform, including Windows. See [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) for
single-test commands, editor setup, and troubleshooting.

Reference-dependent tests compare behavior against a live Redis server and run
only when `RERU_REDIS_REFERENCE_PORT` is set. Set it to the port of a running
Redis instance to enable them. Interoperability checks currently use Redis
7.0.15; the full Redis 8.10.2 behavior suite has not been validated. See
[interoperability testing](docs/INTEROP_TESTING.md).

On Windows, `powershell -File scripts/package_windows.ps1` creates a preview ZIP
and SHA-256 checksum under `target/releases`.

### Project layout

```
src/            server modules (one per subsystem)
tests/          integration tests, one file per area
docs/           compatibility, roadmap, architecture, development, testing
docs/assets/    banner, diagram, and social preview (scripts/gen_readme_assets.py)
scripts/        packaging and Redis interoperability helpers
.cargo/         cargo aliases used by the development guide
.devcontainer/  container and Codespaces setup
```

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for how the pieces fit
together.

</details>

## Contributing

Contributions are welcome. Start with [CONTRIBUTING.md](CONTRIBUTING.md) and the
[roadmap](docs/ROADMAP.md). See who has helped in
[CONTRIBUTORS.md](CONTRIBUTORS.md). Good first issues are labeled
[`good first issue`](https://github.com/mansiverma897993/ReRu/labels/good%20first%20issue).

- Read the [Code of Conduct](CODE_OF_CONDUCT.md) and the
  [governance policy](GOVERNANCE.md), which names the owners and the no-CLA
  contribution terms.
- Report vulnerabilities privately per [SECURITY.md](SECURITY.md).
- Found a bug? [Open a bug report](https://github.com/mansiverma897993/ReRu/issues/new?template=bug_report.yml).
- Have an idea? [Request a feature](https://github.com/mansiverma897993/ReRu/issues/new?template=feature_request.yml).

## Project status

ReRu is suitable for development and compatibility experiments, not production
use. Remaining work includes multi-user ACLs, TLS, RESP3, Lua and functions, newer stream features,
`PSYNC`, clustering and failover, complete RDB/AOF compatibility, and
performance tuning. Memory limits use a logical dataset estimate, not process
RSS. Commands are serialized, and each connection uses an operating-system
thread.

Measured against Redis on one machine, with the method and the limits of the
numbers written down: [docs/BENCHMARKS.md](docs/BENCHMARKS.md). ReRu is faster
on a single connection with shallow pipelining, and slower at 64 connections or
with a deep pipeline.

See the [roadmap](docs/ROADMAP.md) and
[release notes](docs/RELEASE_NOTES.md) for details.

## License

ReRu is released under the
[GNU Affero General Public License v3.0](LICENSE) (`AGPL-3.0-only`).

In plain terms:

- You may use, copy, modify, and self-host ReRu.
- If you distribute a modified version, or run one as a network service, you
  must publish the corresponding source under the same license.
- You may not take ReRu closed source or ship a proprietary fork.

Dependency license notices are in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
Contributions are accepted under the same license; see
[CONTRIBUTING.md](CONTRIBUTING.md).

Copyright (C) 2026 ReRu contributors. See [NOTICE](NOTICE) for the license
notice.

## Acknowledgements

ReRu follows the [Redis command reference](https://redis.io/docs/latest/commands/)
and uses Redis 8.10.2 as its pinned behavior reference. Redis is a trademark of
Redis Ltd. ReRu is an independent project and is not affiliated with or endorsed
by Redis Ltd.
