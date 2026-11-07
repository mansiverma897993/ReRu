# CLI reference

Every startup flag of the `reru` binary, with syntax, default, valid values,
effect, and an example. The source of truth is the `USAGE` string and the
argument parser in `src/main.rs`.

```
usage: reru [--port PORT] [--bind ADDRESS] [--snapshot FILE] [--appendonly FILE]
            [--dir DIR] [--dbfilename FILE] [--replicaof HOST PORT]
            [--requirepass PASSWORD | --requirepass-file FILE]
            [--masterauth PASSWORD | --masterauth-file FILE]
            [--maxmemory SIZE] [--maxmemory-policy POLICY] [--maxclients COUNT]
```

Flags are space-separated (`--port 6380`, not `--port=6380`). Every flag takes a
value except `--help` and `--version`. Unknown flags, or a flag missing its
value, abort startup with the usage text above.

## Quick reference

| Flag | Default | Effect |
| --- | --- | --- |
| `--port PORT` | `6379` | TCP port to listen on |
| `--bind ADDRESS` | `127.0.0.1` | IP address to bind |
| `--snapshot FILE` | off | ReRu snapshot: restore at startup, save after writes |
| `--appendonly FILE` | off | Single-file AOF: replay at startup, append on writes |
| `--dir DIR` | `.` (with RDB flags) | Directory holding the RDB file |
| `--dbfilename FILE` | `dump.rdb` (with RDB flags) | RDB file name |
| `--replicaof HOST PORT` | off | Run as a read-only replica of another ReRu |
| `--requirepass PASSWORD` | off | Require `AUTH` from clients |
| `--requirepass-file FILE` | off | Read the client password from a file |
| `--masterauth PASSWORD` | off | Password used to `AUTH` against the primary |
| `--masterauth-file FILE` | off | Read the primary password from a file |
| `--maxmemory SIZE` | `0` (unlimited) | Dataset memory limit |
| `--maxmemory-policy POLICY` | `noeviction` | Eviction policy |
| `--maxclients COUNT` | `10000` | Maximum concurrent connections |
| `--help`, `-h` | - | Print version and usage |
| `--version`, `-V` | - | Print version |

## `--port PORT`

- **Syntax:** `--port PORT`
- **Default:** `6379`
- **Valid values:** any `u16` port number (`0`-`65535`)
- **Effect:** TCP port the server listens on.
- **Error:** `--port must be a valid port` if the value is not a `u16`.
- **Example:** `reru --port 6380`

## `--bind ADDRESS`

- **Syntax:** `--bind ADDRESS`
- **Default:** `127.0.0.1`
- **Valid values:** any IPv4 or IPv6 address (`0.0.0.0`, `::`, `192.168.1.10`, ...)
- **Effect:** interface the listener binds. `127.0.0.1` keeps the server local;
  bind a wider interface only inside a trusted network.
- **Error:** `--bind must be an IP address` if the value does not parse as an
  `IpAddr` (hostnames are rejected).
- **Example:** `reru --bind 0.0.0.0 --port 6379`

## `--snapshot FILE`

- **Syntax:** `--snapshot FILE`
- **Default:** off
- **Valid values:** any file path
- **Effect:** selects the ReRu snapshot persistence mode. The file is loaded at
  startup if present and rewritten after mutating commands. This is the
  ReRu-specific format, not Redis RDB.
- **Combinations:** cannot be combined with `--appendonly` or the RDB flags
  (`--dir`, `--dbfilename`); can be combined with `--replicaof` so a replica
  persists received snapshots.
- **Example:** `reru --snapshot ./reru.snap`

## `--appendonly FILE`

- **Syntax:** `--appendonly FILE`
- **Default:** off
- **Valid values:** any file path
- **Effect:** selects the single-file AOF persistence mode. Write commands are
  appended and fsynced per batch; the log is replayed at startup.
  `BGREWRITEAOF` compacts it manually in a background worker.
- **Combinations:** cannot be combined with `--snapshot` or `--replicaof`
  (replicas receive state instead of writing an AOF).
- **Example:** `reru --appendonly ./reru.aof`

## `--dir DIR`

- **Syntax:** `--dir DIR`
- **Default:** `.` (the current directory) when any RDB flag is given
- **Valid values:** an existing directory
- **Effect:** directory in which the RDB file is read and written. Only
  meaningful together with `--dbfilename` (or alone, selecting `./dump.rdb`).
- **Combinations:** enables RDB mode; RDB mode cannot be combined with
  `--snapshot` or `--appendonly`.
- **Example:** `reru --dir /var/lib/reru --dbfilename dump.rdb`

## `--dbfilename FILE`

- **Syntax:** `--dbfilename FILE`
- **Default:** `dump.rdb` when any RDB flag is given
- **Valid values:** a bare file name (a single path component, no directories)
- **Effect:** file name of the RDB file inside `--dir`. Loaded at startup if
  present; written by `SAVE` and `BGSAVE`.
- **Error:** `--dbfilename must be a filename` if the value contains a path
  separator.
- **Example:** `reru --dir . --dbfilename dump.rdb`

## `--replicaof HOST PORT`

- **Syntax:** `--replicaof HOST PORT` (two values)
- **Default:** off
- **Valid values:** host name or IP plus a `u16` port of another ReRu server
- **Effect:** starts the server as a read-only replica. It connects to the
  primary, authenticates if `--masterauth` is set, receives a full snapshot and
  then a continuous update stream, and rejects writes with
  `READONLY You can't write against a read only replica.`
- **Errors:** `missing replica port` if the port argument is absent;
  `invalid replica port` if it is not a `u16`. Connection or authentication
  failures are logged as `replica link <address>: <error>` and retried every
  200 ms.
- **Combinations:** cannot be combined with `--appendonly`; may be combined
  with `--snapshot` or the RDB flags to persist received state.
- **Example:** `reru --port 6380 --replicaof 127.0.0.1 6379`

## `--requirepass PASSWORD`

- **Syntax:** `--requirepass PASSWORD`
- **Default:** off (no password; `AUTH` is not required)
- **Valid values:** any byte string; an empty value disables the requirement
- **Effect:** clients must `AUTH <password>` (or `AUTH default <password>`)
  before other commands; otherwise they receive
  `NOAUTH Authentication required.`
- **Example:** `reru --requirepass s3cret`

## `--requirepass-file FILE`

- **Syntax:** `--requirepass-file FILE`
- **Default:** off
- **Valid values:** path to a file of at most 4096 bytes
- **Effect:** reads the client password from a file instead of the command line
  (keeps secrets out of shell history and process lists). A single trailing
  newline (and preceding carriage return) is stripped.
- **Errors:** `password file exceeds 4096 bytes` if the file is too long; the
  OS error if the file cannot be read.
- **Example:** `reru --requirepass-file /etc/reru/pass`

## `--masterauth PASSWORD`

- **Syntax:** `--masterauth PASSWORD`
- **Default:** off
- **Valid values:** any byte string
- **Effect:** password the replica sends as `AUTH` to the primary before
  `REPLSYNC`. Required whenever the primary runs with `--requirepass`.
- **Requirement:** `--masterauth requires --replicaof` - using it without
  `--replicaof` aborts startup. An empty value is treated as unset.
- **Example:** `reru --port 6380 --replicaof 127.0.0.1 6379 --masterauth s3cret`

## `--masterauth-file FILE`

- **Syntax:** `--masterauth-file FILE`
- **Default:** off
- **Valid values:** path to a file of at most 4096 bytes
- **Effect:** `--masterauth` read from a file, with the same trailing-newline
  handling as `--requirepass-file`. Also requires `--replicaof`.
- **Example:** `reru --port 6380 --replicaof 127.0.0.1 6379 --masterauth-file /etc/reru/master.pass`

## `--maxmemory SIZE`

- **Syntax:** `--maxmemory SIZE`
- **Default:** `0` (unlimited)
- **Valid values:** a byte count with an optional case-insensitive unit suffix:
  `b`, `k`/`kb`, `m`/`mb`, `g`/`gb` (for example `64mb`, `1gb`, `524288b`)
- **Effect:** logical dataset memory limit; eviction runs according to
  `--maxmemory-policy` when the limit is reached.
- **Error:** `invalid --maxmemory size` for a non-numeric value or unknown
  suffix.
- **Example:** `reru --maxmemory 64mb --maxmemory-policy allkeys-lru`

## `--maxmemory-policy POLICY`

- **Syntax:** `--maxmemory-policy POLICY`
- **Default:** `noeviction`
- **Valid values:** `noeviction`, `allkeys-lru`, `allkeys-lrm`, `allkeys-lfu`,
  `allkeys-random`, `volatile-lru`, `volatile-lrm`, `volatile-lfu`,
  `volatile-random`, `volatile-ttl` (case-insensitive)
- **Effect:** which keys are evicted when `--maxmemory` is reached.
  `noeviction` rejects writes with an error instead of evicting;
  `volatile-*` policies only consider keys with a TTL.
- **Error:** `invalid --maxmemory-policy` for an unknown policy.
- **Example:** `reru --maxmemory 64mb --maxmemory-policy volatile-ttl`

## `--maxclients COUNT`

- **Syntax:** `--maxclients COUNT`
- **Default:** `10000`
- **Valid values:** a positive integer
- **Effect:** maximum number of concurrent client connections. Further
  connections are answered with `ERR max number of clients reached` and
  closed.
- **Error:** `--maxclients must be a positive integer` for `0` or a
  non-numeric value.
- **Example:** `reru --maxclients 500`

## `--help` / `-h` and `--version` / `-V`

- **Syntax:** `reru --help` or `reru -h`; `reru --version` or `reru -V`
- **Default:** -
- **Effect:** print the version plus usage line (and, for `--help`, a pointer
  to `docs/COMPATIBILITY.md`) and exit without starting the server. Only
  recognized as the sole argument.

## Persistence combinations

Exactly one persistence mode is active. The parser rejects mixed modes at
startup, before the listener binds:

| Combination | Result |
| --- | --- |
| `--snapshot FILE` alone | ReRu snapshot mode |
| `--appendonly FILE` alone | AOF mode |
| `--dir` and/or `--dbfilename` alone | RDB mode (`--dir` defaults to `.`, `--dbfilename` to `dump.rdb`) |
| none of the above | in-memory only |
| `--snapshot` + `--appendonly` | startup error |
| `--snapshot` + `--dir`/`--dbfilename` | startup error |
| `--appendonly` + `--dir`/`--dbfilename` | startup error |
| `--appendonly` + `--replicaof` | startup error |
| `--snapshot` + `--replicaof` | allowed: replica persists received snapshots |
| `--dir`/`--dbfilename` + `--replicaof` | allowed: replica persists received state as RDB |

## Invalid combinations and their error text

Startup aborts with an `InvalidInput` error (message on stderr, non-zero exit)
in these cases:

| Invocation | Error text |
| --- | --- |
| `--masterauth`/`--masterauth-file` without `--replicaof` | `--masterauth requires --replicaof` |
| `--dir`/`--dbfilename` with `--snapshot` or `--appendonly` | `RDB mode cannot be combined with --snapshot or --appendonly` |
| `--appendonly` with `--snapshot` or `--replicaof` | `--appendonly cannot be combined with --snapshot or --replicaof` |
| unknown flag, or flag without value | the full `usage: reru ...` line |
| `--replicaof` without a port | `missing replica port` |
| `--replicaof` with a non-numeric port | `invalid replica port` |
| `--port` with a non-`u16` value | `--port must be a valid port` |
| `--bind` with a hostname | `--bind must be an IP address` |
| `--dbfilename` with a path separator | `--dbfilename must be a filename` |
| `--maxmemory` with a bad value | `invalid --maxmemory size` |
| `--maxmemory-policy` with an unknown policy | `invalid --maxmemory-policy` |
| `--maxclients` with `0` or a non-numeric value | `--maxclients must be a positive integer` |
| password file larger than 4096 bytes | `password file exceeds 4096 bytes` |

## Cross-cutting behavior

- **Flag syntax:** space-separated values only; `--port=6380` is rejected as an
  unknown flag.
- **Case sensitivity:** flag names are case-sensitive; `--maxmemory-policy`
  values are case-insensitive.
- **Replication:** `--replicaof` implies read-only mode. `REPLSYNC` is not
  served on a replica (it answers `unknown command`), so a replica cannot act
  as a primary for further replicas.
- **AUTH gating:** with `--requirepass` set, every command except `AUTH`,
  `QUIT`, `HELLO`, and `RESET` returns `NOAUTH Authentication required.`
  until authentication succeeds.
- **Persistence on replicas:** with `--replicaof` plus a persistence flag,
  received snapshots are written to the configured file; without one, replica
  state is memory-only.
- **Startup order:** arguments are parsed and validated first, then the
  listener binds, then persisted data is loaded, then the replica thread (if
  any) starts. An invalid flag combination aborts before the port is bound.
- **Logging:** the server logs to stderr (`ReRu listening on <ip>:<port>`,
  replica link errors, accept errors); there is no log file flag.
