# Interoperability testing

ReRu is validated in four ways:

1. **Unit and integration tests** that run without external services.
2. **Differential tests** that compare ReRu against a live Redis server.
3. **File interoperability checks** that exchange RDB and AOF files with Redis.
4. **Fuzzing** of the RESP parser, run by hand on a nightly toolchain.

The first group is what CI runs. The others need extra tooling and are run by
hand, or by a maintainer before a compatibility claim.

## Differential tests

Some integration tests compare exact RESP replies with a real Redis. They are
skipped unless `RERU_REDIS_REFERENCE_PORT` is set.

```sh
# Start a dedicated reference server (not on your normal port).
redis-server --port 6391 --save '' --appendonly no --daemonize yes

RERU_REDIS_REFERENCE_PORT=6391 cargo test

# Only the reference-heavy suites:
RERU_REDIS_REFERENCE_PORT=6391 cargo test \
  --test redis_differential --test rdb --test stream_groups \
  --test hyperloglog --test connection
```

On PowerShell:

```powershell
$env:RERU_REDIS_REFERENCE_PORT = '6391'
cargo test --test redis_differential --test rdb --test stream_groups --test hyperloglog --test connection
Remove-Item Env:RERU_REDIS_REFERENCE_PORT
```

Use a dedicated instance. The tests write keys and may flush databases.

## RDB interoperability

`scripts/test_rdb_interop.py` checks the full exchange path:

```
Redis -> RDB file -> ReRu load -> ReRu SAVE -> RDB file -> Redis load
```

It creates temporary servers and files, compares values, and validates the
exported file with `redis-check-rdb`.

```sh
python scripts/test_rdb_interop.py --reru target/debug/reru
```

On Windows, run it against a Linux binary through WSL:

```powershell
python scripts/test_rdb_interop.py --wsl Ubuntu --reru target/debug/reru.exe
```

## AOF interoperability

`scripts/test_aof_rewrite_interop.py` compacts ReRu's append-only file, checks it
with `redis-check-aof`, loads it into a dedicated Redis server, and compares
values, expiry deadlines, stream IDs, consumer groups, pending ownership,
deleted and trimmed payloads, explicit ID resets, HyperLogLog strings, and
writes issued after rewrite began.

```sh
python -B scripts/test_aof_rewrite_interop.py --reru target/debug/reru
```

## Fuzzing the RESP parser

`src/protocol.rs` reads the first bytes a client sends, so it gets a fuzz target
of its own. The target lives in `fuzz/`, a separate Cargo workspace that the
repository build does not include. **CI does not run fuzzing**; it needs a
nightly toolchain and runs for however long an operator lets it.

Fuzzing uses [`cargo-fuzz`](https://github.com/rust-fuzz/cargo-fuzz), which is
not part of the repository:

```sh
rustup toolchain install nightly
cargo install cargo-fuzz
```

Run the target from the `fuzz/` directory:

```sh
cd fuzz
cargo +nightly fuzz run resp_parser

# Or bound it, which is enough for a smoke check.
cargo +nightly fuzz run resp_parser -- -max_total_time=60
```

The target parses up to eight commands per input and asserts the limits the
server enforces: at most 1024 arguments, and at most 64 MiB per argument. The
per-command total is not reachable from a fuzz input, so
`protocol::tests::rejects_a_command_whose_arguments_exceed_the_total_limit`
covers it instead. Any panic fails the run.

`fuzz/corpus/resp_parser/` holds the seed corpus, which is committed and covers
arrays, bulk-string lengths, two frames in one input, and truncated frames. It
also holds inline commands and bad prefixes, which the parser rejects; ReRu has
no inline command mode. `.gitattributes` marks the directory `-text`; CRLF is
framing here, not a line ending, so the files must not be normalized. libFuzzer
adds discovered inputs to the corpus as it runs; commit useful ones. A crash is
written to `fuzz/artifacts/resp_parser/`; reproduce it with:

```sh
cd fuzz
cargo +nightly fuzz run resp_parser artifacts/resp_parser/crash-<hash>
```

`cargo fuzz` builds with a sanitizer. On Linux and macOS the default address
sanitizer finds overflows and use-after-free as well as panics.

`fuzz/` is its own workspace, so the repository linters do not see it. Check it
from inside the directory when changing the target:

```sh
cd fuzz
cargo +nightly fmt --check
cargo +nightly clippy --all-targets -- -D warnings
```

## Reference version

The tests currently use **Redis 7.0.15** as the reference. The compatibility
target is **Redis 8.10.2**. Run the suite against the pinned version before
claiming coverage for its newer encodings and behaviors.

## Command-name coverage

`scripts/command_coverage.ps1` counts command names with an implementation path
against the pinned Redis metadata:

```powershell
powershell -File scripts/command_coverage.ps1 -RedisSource PATH_TO_REDIS_8_10_2
```

This is a name-presence count, not a behavior score. A command listed there may
still reject options or return different errors.

## Reporting results

When a change touches a command reply, error, expiry, or persistence, include
the commands you ran in the pull request. If you find a difference, add a
regression test before fixing it.
