# Contributing to ReRu

Thanks for helping. ReRu is an experimental Redis-compatible server, and every
change should keep the server usable and the test suite green.

This guide covers the workflow. By participating you agree to the
[Code of Conduct](CODE_OF_CONDUCT.md).

## Ways to contribute

- **Implement a command.** Pick an unimplemented entry from the
  [compatibility inventory](docs/COMPATIBILITY.md) or the
  [roadmap](docs/ROADMAP.md).
- **Fix a compatibility gap.** Run a Redis comparison, find a difference, add a
  regression test, then fix it.
- **Improve docs.** Corrections to the compatibility notes are especially
  valuable.
- **Report bugs.** Use the issue template and include your OS, Rust version,
  and exact commands.

If you plan a large change, open an issue first so we can agree on the design.

The [open-source playbook](OSS_PLAYBOOK.md) lists the current backlog, the
programs we take part in, and the launch plan.

## Development setup

Requirements:

- Rust 1.88 or newer (`rustup` recommended; `rust-toolchain.toml` pins stable)
- A C toolchain (standard for Rust builds)
- Optional: `make` for the convenience targets
- Optional: `redis-server` and `redis-cli` for differential tests

```sh
git clone https://github.com/mansiverma897993/ReRu.git
cd ReRu
make setup      # or: cargo fetch
make test       # or: cargo test
make run        # or: cargo run -- --port 6379
```

`make` is not required. The `cargo` aliases in `.cargo/config.toml` work on
every platform, including Windows PowerShell.

The full guide, with single-test commands, editor setup, and troubleshooting,
is in [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md).

## Before you open a pull request

Run all four checks locally. CI runs the same commands:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build --release
```

All must pass. Do not disable lints or skip tests to make CI green.

### Differential tests against Redis

Some tests compare ReRu with a live Redis server:

```sh
redis-server --port 6391 --save '' --appendonly no --daemonize yes
RERU_REDIS_REFERENCE_PORT=6391 cargo test
```

If the variable is unset, those tests skip. Please run them when your change
touches command replies, errors, expiry, or persistence.

## Adding a command

1. **Read the Redis 8.10.2 documentation** for the command, including errors,
   arity, and option edge cases.
2. **Choose the module.** Commands live in the `src/` module that owns their
   data type, for example `src/list_ops.rs` or `src/sorted_sets.rs`. Add the
   dispatch arm in `src/main.rs`.
3. **Write the integration test first** in the matching `tests/` file. Test the
   happy path, the error replies, binary safety, and expiry where relevant.
4. **Implement the command.** Reuse the existing helpers (`protocol::`, store
   guard, `Value` variants) instead of adding parallel machinery.
5. **Update the docs.** Add the command to `docs/COMPATIBILITY.md` and, if the
   behavior differs from Redis, record the difference.
6. **Run the full suite**, then open the pull request.

### Test conventions

- One integration test file per area (`tests/cache.rs`, `tests/streams.rs`).
- Start a fresh server per test with `common::Server::start()`.
- Compare exact RESP replies, not just success.
- Keep tests deterministic. Do not depend on wall-clock timing where a
  condition variable or retry loop will do.

## Code style

- `rustfmt` is authoritative. Run `cargo fmt`.
- Clippy runs with `-D warnings`.
- Match the surrounding code. Modules are plain functions plus small structs;
  no async runtime, no new dependencies without discussion.
- Keep a request path allocation-light and keep blocking work off the client
  thread where the existing design already does so.
- Prefer clear names and short functions over comments that restate the code.
- Comments should explain *why*, especially for Redis quirks.

## Commit messages

Use a short imperative subject line, then a body explaining the behavior change
and how it was tested:

```
Add ZRANGESTORE and cover lex bounds

Implements ZRANGESTORE for score and lex ranges, including the
REV and LIMIT options. Adds integration tests for empty results,
binary members, and destination overwrite.

Tested with: cargo test; Redis 7.0.15 differential run.
```

Group related changes in one commit. Keep unrelated refactors separate.

## Pull requests

- One focused change per pull request.
- Link the issue it closes (`Closes #123`).
- Fill in the pull request template: what changed, how it was tested, and any
  known differences from Redis.
- Keep the diff reviewable. Split large work into stacked pull requests.
- An owner reviews, or a maintainer once one joins the project, for correctness,
  test coverage, and compatibility notes.
- Do not force-push after review starts unless asked; add follow-up commits.

## Getting credit for your work

Contributors are listed in [CONTRIBUTORS.md](CONTRIBUTORS.md), maintained with
the [all-contributors](https://allcontributors.org) spec (`.all-contributorsrc`).
Once your first PR lands, a maintainer or the all-contributors bot can add you;
comment `@all-contributors please add @<username> for <contributions>` on any
issue or PR.

## Reporting bugs and security issues

- Functional bugs: open an issue with the bug report template.
- Security issues: **do not open a public issue.** Follow
  [SECURITY.md](SECURITY.md).

## Who decides

The owners, the decision rule, the no-CLA terms, and the release process are in
[GOVERNANCE.md](GOVERNANCE.md).

## License

ReRu is licensed under the GNU Affero General Public License v3.0
(AGPL-3.0-only).

By submitting a pull request you agree that your contribution is licensed under
the same terms. No contributor license agreement is required.

### Clean-room rule

Redis is licensed under AGPLv3. ReRu must stay a clean-room implementation:

- Implement behavior from the Redis documentation, the protocol specification,
  and observed responses from a running server.
- Do not copy Redis source code, copy from Redis forks, or translate Redis
  source line by line.
- Do not paste code from a language model that was asked to reproduce Redis
  functions.
- If you are unsure whether a reference is acceptable, ask in the issue first.

This rule protects every contributor and keeps the project's license valid.
