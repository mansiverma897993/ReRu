# CLI reference

Every startup flag of the `reru` binary with its default in the table below and
a one-line description underneath. The source of truth is the `USAGE` string and
the argument parser in `src/main.rs`.

```
usage: reru [--port PORT] [--bind ADDRESS] [--snapshot FILE] [--appendonly FILE]
            [--dir DIR] [--dbfilename FILE] [--replicaof HOST PORT]
            [--requirepass PASSWORD | --requirepass-file FILE]
            [--masterauth PASSWORD | --masterauth-file FILE]
            [--maxmemory SIZE] [--maxmemory-policy POLICY] [--maxclients COUNT]
```

Flags are space-separated (`--port 6380`, not `--port=6380`). Every flag takes a
value except `--help` and `--version`, which are only recognized as the sole
argument. Unknown flags, or a flag missing its value, abort startup with the
usage text above.

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

## Flag details

- `--port PORT` — TCP port to listen on; any `u16` value.
- `--bind ADDRESS` — interface to bind; IPv4 or IPv6 address, hostnames are
  rejected. Widen beyond `127.0.0.1` only in a trusted network.
- `--snapshot FILE` — ReRu snapshot mode: restore the file at startup and save
  after mutating commands. ReRu-specific format, not Redis RDB.
- `--appendonly FILE` — single-file AOF mode: append and fsync writes, replay at
  startup, compact manually with `BGREWRITEAOF`.
- `--dir DIR`, `--dbfilename FILE` — enable RDB mode: read the file at startup,
  write it with `SAVE` and `BGSAVE`. `--dbfilename` must be a bare file name.
- `--replicaof HOST PORT` — read-only replica: authenticate with `--masterauth`
  when set, receive a snapshot plus update stream, reject writes with
  `READONLY You can't write against a read only replica.`
- `--requirepass PASSWORD` — clients must `AUTH <password>` before other
  commands; an empty value disables the requirement.
- `--requirepass-file FILE` — client password from a file (max 4096 bytes, one
  trailing line ending stripped).
- `--masterauth PASSWORD` — password the replica sends to the primary; requires
  `--replicaof`.
- `--masterauth-file FILE` — primary password from a file; requires
  `--replicaof`.
- `--maxmemory SIZE` — logical dataset memory limit; byte counts accept
  case-insensitive `b`, `k`, `m`, `g` suffixes (`64mb`, `1gb`). Not process RSS.
- `--maxmemory-policy POLICY` — eviction policy once `--maxmemory` is reached;
  `noeviction` rejects writes with an error instead of evicting. Valid values:
  `noeviction`, `allkeys-lru`, `allkeys-lrm`, `allkeys-lfu`, `allkeys-random`,
  `volatile-lru`, `volatile-lrm`, `volatile-lfu`, `volatile-random`,
  `volatile-ttl` (case-insensitive).
- `--maxclients COUNT` — maximum concurrent connections; excess connections get
  `ERR max number of clients reached` and are closed.
- `--help`, `-h`, `--version`, `-V` — print the version plus usage line and exit
  without starting the server. They are only recognized as the sole argument;
  passed with any other flag they abort startup with the usage text.

## Notes

- `--snapshot`, `--appendonly`, and the RDB flags are three mutually exclusive
  persistence modes; mixing them aborts startup with an `InvalidInput` error
  before the listener binds. `--appendonly` cannot combine with `--replicaof`;
  `--snapshot` and the RDB flags may, so a replica persists received state.
- `--masterauth` or `--masterauth-file` without `--replicaof` aborts startup
  with `--masterauth requires --replicaof`.
- With `--requirepass` set, every command except `AUTH`, `QUIT`, `HELLO`, and
  `RESET` returns `NOAUTH Authentication required.` until authentication
  succeeds.
- Flag names are case-sensitive; `--maxmemory-policy` values are not.
