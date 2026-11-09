# Architecture

This document explains how ReRu is put together, so a new contributor can find
the right file before changing behavior. The startup flags of the binary are
documented in [CLI.md](CLI.md).

## Process model

ReRu is a single binary with a deliberately small runtime:

- **One thread per client connection.** `main` accepts a TCP stream, acquires a
  client-limit permit, and spawns a thread named `client`.
- **One shared store.** All connections hold the same
  `Store = Arc<SharedStore>`. State lives behind `Mutex` guards inside it.
- **One command gate.** `SharedStore::command_gate` is a `Mutex<()>` that
  serializes command execution. Redis executes commands one at a time; ReRu
  reproduces that property without an event loop.
- **No async runtime.** Blocking commands park on a `Condvar`
  (`SharedStore::changed`) instead of an async reactor.

The tradeoff is intentional: the code stays readable at the cost of one OS
thread per connection. See [Project status](../README.md#project-status).

## Data model

`src/main.rs` owns the core types:

```rust
enum Value {
    String(Vec<u8>),
    List(Vec<Vec<u8>>),
    Stream(streams::Stream),
    Hash(HashMap<Vec<u8>, Vec<u8>>),
    Set(HashSet<Vec<u8>>),
    SortedSet(sorted_sets::SortedSet),
}

struct Entry {
    value: Value,
    expires_at: Option<Instant>,
}
```

Keys are binary-safe `Vec<u8>`. Expiry uses monotonic `Instant`, not wall-clock
time, so clock changes do not extend or shorten a TTL.

`SharedStore` also holds the persistence paths, the replica senders, watch
registrations, eviction state, expiry index, client limits, AOF rewrite state,
authentication settings, and the pub/sub hub.

## Module map

| Module | Responsibility |
| --- | --- |
| `main.rs` | Core types, command dispatch, blocking helpers, client loop, startup |
| `protocol.rs` | RESP2 reader and reply writers; size and argument limits |
| `connection.rs` | `HELLO`, `SELECT`, `CLIENT`, `RESET`, per-socket metadata |
| `cache.rs` | String and cache commands, conditional writes, expiry on access |
| `list_ops.rs` | List commands and blocking pops/moves |
| `collections.rs` | Hash and set commands |
| `sorted_sets.rs` | Sorted sets, ranges, and blocking pops |
| `sorted_sets/algebra.rs` | `ZUNION`, `ZINTER`, `ZDIFF` and store variants |
| `bitmaps.rs` | Bit operations and `BITFIELD` |
| `streams.rs` | Stream storage, IDs, ranges, trimming |
| `stream_groups.rs` | Consumer groups, pending entries, claiming, `XINFO` |
| `geo.rs` | Geohash encoding, distance, radius and box search |
| `hyperloglog.rs` | HYLL interchange and approximate counting |
| `keyspace.rs` | `DEL`, `EXISTS`, `SCAN`, `RENAME`, `COPY`, expiry inspection |
| `expiry.rs` | Deadline index and the background expiry thread |
| `eviction.rs` | Logical memory estimate and the ten eviction policies |
| `clients.rs` | Connection limit permits and connection counters |
| `pubsub.rs` | Channel and pattern subscriptions, delivery hub |
| `persistence.rs` | ReRu snapshot format (save, load, frames) |
| `aof.rs` | Append-only file: replay, append, background rewrite |
| `rdb.rs` | Redis RDB subset: load, `SAVE`, `BGSAVE`, `DUMP`, `RESTORE` |
| `replication.rs` | Replica bookkeeping and `INFO` role reporting |
| `auth.rs` | Default-user password settings and password-file parsing |

Each module owns its command handlers as `pub fn` functions that take the
parsed command and the store, and return raw RESP bytes.

## Command path

1. `handle_client` reads one command with `protocol::read_command`.
2. Session-scoped commands (`HELLO`, `CLIENT`, `RESET`, `AUTH`) are handled by
   `connection::Session` before the dataset is touched.
3. `MULTI` queues commands, validates arity, and replays the queue on `EXEC`
   while holding the command gate.
4. Blocking commands (`BLPOP`, `XREAD`, `BZPOPMIN`, …) take a separate path that
   waits on the condition variable with a deadline.
5. Everything else goes through `execute_command`, a dispatch over the command
   name that delegates to the owning module.
6. After a mutating command, `record_write` updates watch registrations,
   `is_mutating` decides whether persistence runs, and `save_and_publish`
   writes the snapshot/AOF and forwards the change to replicas.

## Persistence and replication pipeline

`save_and_publish` is the single write-out point:

- **Snapshot mode** writes the whole dataset through `persistence.rs`.
- **AOF mode** appends the RESP transaction through `aof.rs`, then fsyncs
  according to the configured mode.
- **RDB mode** does not write on every change; `SAVE` and `BGSAVE` call `rdb.rs`.
- **Replication** broadcasts a snapshot frame on first connect, then one frame
  per write, to every registered replica sender.

AOF replay runs at startup before the listener serves traffic. Background AOF
rewrite snapshots the dataset, records writes that arrive during the rewrite,
and appends them to the compacted file.

## Expiry

`expiry.rs` keeps one deadline record per expiring key, so repeated `EXPIRE`
calls replace the record instead of accumulating timers. A background thread
wakes on the earliest deadline and deletes in bounded batches. Reads also check
`expires_at` lazily, which covers keys whose background deletion has not run
yet.

## Eviction

`eviction.rs` computes a logical dataset estimate and, when it exceeds
`--maxmemory`, applies the configured policy. The estimate is not process RSS;
allocator overhead and connection buffers are excluded. `noeviction` returns an
error instead of deleting keys.

## Concurrency rules

- Take `command_gate` before mutating shared state.
- Do not hold a lock while blocking on `changed`; wait on the condition
  variable and re-check.
- Notify `changed` after any write that could unblock a waiter.
- Blocking paths must return immediately inside `MULTI`/`EXEC`.

## Testing structure

Integration tests live in `tests/`, one file per area (`cache.rs`, `streams.rs`,
`rdb.rs`, …). Each test starts a real server process through
`tests/common/mod.rs`, speaks raw RESP through `tests/common/client.rs`, and
asserts exact replies. Reference-dependent tests run only when
`RERU_REDIS_REFERENCE_PORT` points at a live Redis server.

## Where to add a command

1. Handler in the module that owns the data type.
2. Dispatch arm in `execute_command` in `main.rs`.
3. Mutating classification in `is_mutating` if the command changes state.
4. Integration test in the matching `tests/` file.
5. Entry in `docs/COMPATIBILITY.md`.
