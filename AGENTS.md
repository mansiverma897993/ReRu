# AGENTS.md

Guidance for coding agents and AI-assisted contributors working in this
repository. Human contributors should read [CONTRIBUTING.md](CONTRIBUTING.md).

## Ground truth

- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) — module map and command path.
  Read it before changing `src/main.rs`.
- [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) — setup, commands, and troubleshooting.
- [docs/COMPATIBILITY.md](docs/COMPATIBILITY.md) — what is implemented, what
  differs from Redis, and what is untested.
- [docs/ROADMAP.md](docs/ROADMAP.md) — priorities.

## Rules

1. **Match Redis 8.10.2 behavior.** Cite the Redis documentation for the command
   in the PR. If an option is unsupported, say so in the compatibility doc
   instead of silently returning a different reply.
2. **No new dependencies** without an issue and maintainer agreement. The crate
   has one runtime dependency by design.
3. **No async runtime.** Commands run under the shared `command_gate`; blocking
   commands wait on the `changed` condition variable.
4. **No `unsafe`.** Both crates declare `#![forbid(unsafe_code)]`, which an
   `#[allow]` cannot relax. A change that needs `unsafe` needs a maintainer
   decision to change the attribute first.
5. **Test before claiming done.** A command is not implemented until an
   integration test in `tests/` asserts its exact RESP replies and at least one
   error case.
6. **Update docs with the code.** `docs/COMPATIBILITY.md` for command changes,
   `CHANGELOG.md` under `Unreleased` for user-visible changes.
7. **Surgical diffs.** Do not refactor unrelated modules, rename public items, or
   reformat files you did not change.

## Verification commands

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
```

Or run `make check`, which executes all three. Extensive changes should also run
`cargo build --release`.

## Where code goes

| Change | File |
| --- | --- |
| String or cache command | `src/cache.rs` |
| List command | `src/list_ops.rs` |
| Hash or set command | `src/collections.rs` |
| Sorted-set command | `src/sorted_sets.rs` |
| Multi-key sorted-set algebra | `src/sorted_sets/algebra.rs` |
| Stream or consumer-group command | `src/streams.rs`, `src/stream_groups.rs` |
| Bitmap command | `src/bitmaps.rs` |
| Geo command | `src/geo.rs` |
| Keyspace command | `src/keyspace.rs` |
| Protocol parsing or reply helpers | `src/protocol.rs` |
| New startup flag | `src/main.rs` argument loop and `USAGE` |
| Dispatch | `execute_command` and `is_mutating` in `src/main.rs` |

## Do not

- Copy, paste, or translate Redis source code or code from a Redis fork. ReRu is
  a clean-room implementation; implement from documentation and observed
  behavior only.
- Commit secrets, generated data files (`*.aof`, `*.rdb`, `*.snap`), or
  machine-specific paths.
- Add `Co-authored-by` or tool attribution trailers to commits.
- Disable tests, lints, or `-D warnings` to make CI pass.
- Rewrite `docs/COMPATIBILITY.md` coverage claims without running the referenced
  tests.
