# Development guide

Everything needed to build, run, and test ReRu on a fresh machine.

## What you need

| Tool | Version | Notes |
| --- | --- | --- |
| Rust | 1.88 or newer | Install with [rustup](https://rustup.rs). `rust-toolchain.toml` handles the rest |
| A C toolchain | any | `cc` or `clang`; standard Rust builds need it |
| `make` | optional | Convenience targets. On Windows, use the `cargo` aliases instead |
| Redis | optional | Only for differential tests (`redis-server`, `redis-cli`) |

ReRu has one runtime dependency (`subtle`), so the first build is short.

## 60-second setup

```sh
git clone https://github.com/mansiverma897993/ReRu.git
cd ReRu
make setup      # or: cargo fetch
make test       # or: cargo test
make run        # or: cargo run -- --port 6379
```

The server prints `ReRu listening on 127.0.0.1:6379`.

If `make` is not installed, use the `cargo` aliases from `.cargo/config.toml`:

```sh
cargo fetch
cargo test
cargo serve --port 6379
```

## Daily commands

| Task | `make` | Without `make` |
| --- | --- | --- |
| Fetch dependencies | `make setup` | `cargo fetch` |
| Debug build | `make build` | `cargo build` |
| Run the server | `make run` | `cargo run -- --port 6379` |
| Run on another port | `make run ARGS="--port 6380"` | `cargo run -- --port 6380` |
| Run all tests | `make test` | `cargo test` |
| Format | `make fmt` | `cargo fmt --all` |
| Check formatting | `make fmt-check` | `cargo fmt-check` |
| Lint | `make lint` | `cargo lint` |
| Everything before a PR | `make check` | `cargo fmt-check && cargo lint && cargo test` |
| Release build | `make release` | `cargo build --release` |

## Try it

In a second terminal:

```sh
redis-cli -p 6379 SET greeting "hello" EX 60
redis-cli -p 6379 GET greeting
redis-cli -p 6379 TTL greeting
redis-cli -p 6379 RPUSH queue a b c
redis-cli -p 6379 LRANGE queue 0 -1
```

No `redis-cli`? Install Redis, or write a raw RESP client. The protocol is
documented in [`docs/ARCHITECTURE.md`](ARCHITECTURE.md) and in
[`src/protocol.rs`](../src/protocol.rs).

## Tests

The suite has 221 tests. One file per area:

```
tests/cache.rs          strings and cache commands
tests/list_ops.rs       lists and blocking pops
tests/collections.rs    hashes and sets
tests/sorted_sets.rs    sorted sets
tests/streams.rs        streams
tests/persistence.rs    snapshot, AOF, RDB
tests/replication.rs    replicas
tests/connection.rs     HELLO, CLIENT, RESET
```

Each test starts a real server process on an operating-system-assigned port.
Run one test:

```sh
cargo test --test cache -- --nocapture set_and_get
```

Run one file:

```sh
cargo test --test sorted_sets
```

### Differential tests against Redis

These compare exact replies with a reference server and skip when the variable
is unset:

```sh
make redis                                  # starts Redis on port 6391
make test-redis                             # or: RERU_REDIS_REFERENCE_PORT=6391 cargo test
make redis-stop
```

See [`docs/INTEROP_TESTING.md`](INTEROP_TESTING.md) for the RDB and AOF file
exchange scripts.

## Troubleshooting

**`package reru requires rustc 1.88`**
Run `rustup update stable`. Confirm with `rustc --version`.

**Port 6379 is already in use**
Run on another port: `make run ARGS="--port 6380"`, or check with
`lsof -i :6379` (macOS/Linux) or `netstat -ano | findstr 6379` (Windows).

**`redis-cli: command not found`**
Install Redis. On macOS: `brew install redis`. On Debian/Ubuntu:
`sudo apt-get install redis-tools`. The Rust tests do not need it.

**Differential tests report `skipped`**
That is expected without `RERU_REDIS_REFERENCE_PORT`. Start the reference
server with `make redis`.

**Clippy fails after a Rust update**
A newer stable may add lints. Fix the lint or open an issue. Do not add
`#[allow(...)]` to hide a real problem.

**The first `cargo test` is slow**
The first run compiles the test binaries. Later runs take seconds.

**Windows**
The `cargo` aliases work in PowerShell. `make` needs Git Bash, MSYS2, or WSL.
Path examples in this repo use Unix separators; use `\` in PowerShell.

## Editor setup

- **VS Code / Cursor:** install `rust-analyzer`, `CodeLLDB`, and `Even Better
  TOML`. The repo recommends them in `.vscode/extensions.json`.
- **Dev container / Codespaces:** open the repo in a container. The
  `.devcontainer/devcontainer.json` forwards port 6379 and enables Clippy on
  save.
- **RustRover / IntelliJ:** open the folder; the Cargo project is detected.

## Where to start

1. Read [`docs/ARCHITECTURE.md`](ARCHITECTURE.md) for the module map.
2. Pick an issue labelled
   [`good first issue`](https://github.com/mansiverma897993/ReRu/labels/good%20first%20issue).
3. Comment to claim it.
4. Write the test first, then the code.
5. Run `make check`.
6. Open a pull request.

Open tasks are in the [issue tracker](https://github.com/mansiverma897993/ReRu/issues); start with `good first issue`.
