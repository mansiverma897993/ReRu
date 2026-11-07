<div align="center">

<img src="docs/assets/banner.svg" alt="ReRu — a Redis-compatible data server written in Rust" width="100%">

<br>

[![CI](https://img.shields.io/github/actions/workflow/status/mansiverma897993/ReRu/ci.yml?style=flat-square&label=CI&logo=githubactions&logoColor=white)](https://github.com/mansiverma897993/ReRu/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-MIT-3DA639?style=flat-square)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.88%2B-DEA584?style=flat-square&logo=rust&logoColor=white)](https://www.rust-lang.org)
[![Status](https://img.shields.io/badge/status-experimental-F97316?style=flat-square)](docs/ROADMAP.md)
[![PRs](https://img.shields.io/badge/PRs-welcome-6E56CF?style=flat-square)](CONTRIBUTING.md)
![Platforms](https://img.shields.io/badge/platforms-linux%20%7C%20macos%20%7C%20windows-2F81F7?style=flat-square)

<p><b>ReRu speaks RESP2, so existing Redis clients connect without changes.</b><br>
It reimplements a growing slice of Redis and its persistence formats in safe, dependency-light Rust.</p>

[**Quick start**](#quick-start) ·
[**Compatibility**](docs/COMPATIBILITY.md) ·
[**Architecture**](docs/ARCHITECTURE.md) ·
[**Roadmap**](docs/ROADMAP.md) ·
[**Contributing**](CONTRIBUTING.md)

</div>

> [!WARNING]
> **Experimental preview (`0.1.0-alpha.1`).** ReRu is built for development,
> compatibility experiments, and learning. It is **not** a production Redis
> replacement. See [Project status](#project-status).

## At a glance

<table>
  <tr>
    <td align="center" width="25%"><strong>203</strong><br><sub>Redis command names wired</sub></td>
    <td align="center" width="25%"><strong>204</strong><br><sub>tests passing</sub></td>
    <td align="center" width="25%"><strong>1</strong><br><sub>runtime dependency</sub></td>
    <td align="center" width="25%"><strong>10</strong><br><sub>eviction policies</sub></td>
  </tr>
</table>

<p align="center"><sub>44.2% command-name coverage against Redis 8.10.2 metadata. Name presence, not complete behavior.</sub></p>

## Why ReRu

<table>
  <tr>
    <td align="center" width="33%" valign="top">
      <h3>🧩</h3>
      <strong>Familiar protocol</strong><br>
      <sub>Binary-safe RESP2 over TCP. Works with <code>redis-cli</code>, <code>redis-py</code>, <code>ioredis</code>, Lettuce, and any client that supports the listed commands.</sub>
    </td>
    <td align="center" width="33%" valign="top">
      <h3>🪶</h3>
      <strong>Small and readable</strong><br>
      <sub>About 21k lines of straight-line Rust with one dependency. Every subsystem is a plain module with its own tests.</sub>
    </td>
    <td align="center" width="33%" valign="top">
      <h3>🔬</h3>
      <strong>Research-friendly</strong><br>
      <sub>Each command follows the pinned Redis 8.10.2 reference, with differential tests against a live Redis where possible.</sub>
    </td>
  </tr>
</table>

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

<table>
  <tr>
    <td align="center" width="33%" valign="top">
      <h3>🧱</h3>
      <strong>Data structures</strong><br>
      <sub>Strings, lists, hashes, sets, sorted sets, bitmaps, streams with consumer groups, and geospatial queries.</sub>
    </td>
    <td align="center" width="33%" valign="top">
      <h3>⚡</h3>
      <strong>Cache behavior</strong><br>
      <sub>Conditional writes, TTLs, background expiry, ten eviction policies, and configurable memory estimates.</sub>
    </td>
    <td align="center" width="33%" valign="top">
      <h3>🔁</h3>
      <strong>Transactions &amp; messaging</strong><br>
      <sub><code>MULTI</code>, <code>EXEC</code>, <code>WATCH</code>, blocking reads, local pub/sub, stream acknowledgements, and pending-message recovery.</sub>
    </td>
  </tr>
  <tr>
    <td align="center" width="33%" valign="top">
      <h3>💾</h3>
      <strong>Persistence</strong><br>
      <sub>ReRu snapshots, a single-file AOF with online rewrite, and a tested subset of Redis RDB.</sub>
    </td>
    <td align="center" width="33%" valign="top">
      <h3>🛰️</h3>
      <strong>Replication</strong><br>
      <sub>Read-only ReRu replicas using an authenticated snapshot-plus-stream protocol.</sub>
    </td>
    <td align="center" width="33%" valign="top">
      <h3>🔐</h3>
      <strong>Auth &amp; operations</strong><br>
      <sub>Default-user <code>AUTH</code>, client limits, runtime memory configuration, and partial <code>INFO</code> metrics.</sub>
    </td>
  </tr>
</table>

**203 of 459 command metadata entries** from Redis 8.10.2 have implementation
paths: **44.2% command-name coverage**. This counts command names, not complete
behavior. Supported options, differences, and tests are listed in the
[compatibility inventory](docs/COMPATIBILITY.md).

## Architecture

<img src="docs/assets/architecture.svg" alt="ReRu request path: clients, RESP2 parser, command dispatch, shared store, then snapshots, AOF, RDB, and replicas" width="100%">

One connection thread per client. Writes are serialized by the command gate and
fan out from a single write-out point. Read the full
[architecture notes](docs/ARCHITECTURE.md) for the module map and the command
path.

## Server options

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

## Development

<details>
<summary><b>Commands, layout, and test setup</b></summary>

<br>

```sh
cargo build                    # debug build
cargo test                     # 204 tests
cargo fmt --all -- --check     # formatting
cargo clippy --all-targets -- -D warnings   # lints
```

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
docs/           compatibility, roadmap, architecture, testing notes
docs/assets/    banner, diagram, and social preview
scripts/        packaging and Redis interoperability helpers
```

See [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for how the pieces fit
together.

</details>

## Contributing

Contributions are welcome. Start with [CONTRIBUTING.md](CONTRIBUTING.md) and the
[roadmap](docs/ROADMAP.md). Good first issues are labeled
[`good first issue`](https://github.com/mansiverma897993/ReRu/labels/good%20first%20issue).

- Read the [Code of Conduct](CODE_OF_CONDUCT.md).
- Report vulnerabilities privately per [SECURITY.md](SECURITY.md).
- Claiming a command? Add it to the [compatibility inventory](docs/COMPATIBILITY.md)
  and cover it with an integration test in `tests/`.

<table>
  <tr>
    <td align="center" width="33%" valign="top">
      <strong>🐛 Found a bug?</strong><br>
      <sub><a href="https://github.com/mansiverma897993/ReRu/issues/new?template=bug_report.yml">Open a bug report</a></sub>
    </td>
    <td align="center" width="33%" valign="top">
      <strong>💡 Have an idea?</strong><br>
      <sub><a href="https://github.com/mansiverma897993/ReRu/issues/new?template=feature_request.yml">Request a feature</a></sub>
    </td>
    <td align="center" width="33%" valign="top">
      <strong>🔒 Found a vulnerability?</strong><br>
      <sub><a href="https://github.com/mansiverma897993/ReRu/security/advisories/new">Report it privately</a></sub>
    </td>
  </tr>
</table>

## Project status

ReRu is suitable for development and compatibility experiments. Remaining work
includes multi-user ACLs, TLS, RESP3, Lua and functions, newer stream features,
`PSYNC`, clustering and failover, complete RDB/AOF compatibility, and
performance tuning. Memory limits use a logical dataset estimate, not process
RSS. Commands are serialized, and each connection uses an operating-system
thread.

See the [roadmap](docs/ROADMAP.md) and
[release notes](docs/RELEASE_NOTES.md) for details.

## License

ReRu is released under the [MIT License](LICENSE). Dependency license notices
are in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

## Acknowledgements

ReRu follows the [Redis command reference](https://redis.io/docs/latest/commands/)
and uses Redis 8.10.2 as its pinned behavior reference. Redis is a trademark of
Redis Ltd. ReRu is an independent project and is not affiliated with or endorsed
by Redis Ltd.
