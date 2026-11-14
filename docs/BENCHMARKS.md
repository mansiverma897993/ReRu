# Benchmarks

This page records how ReRu compares with Redis on one machine, using one method,
so the numbers can be reproduced and argued with.

Read the [what this does not prove](#what-this-does-not-prove) section before
quoting any number from this page.

## Method

Both servers are built for release, started on the same host with persistence
off, and otherwise left at their defaults. `redis-benchmark`, which ships with
Redis, drives both with the same arguments. Only the port differs.

The harness:

- discards a warmup pass for every configuration
- runs each configuration three times by default and reports the median
- alternates which server runs first, so drift is shared rather than charged to
  one server; with an odd run count one server still gets the earlier slot more
  often
- reports throughput in requests per second, and client-side p50 and p99 latency
- reads RSS from the listening process after the run, when `lsof` and `ps` exist

No CPU pinning and no kernel tuning are used. On this host that is not possible;
on Linux, pin both servers to separate cores before comparing.

```sh
cargo build --release

redis-server --port 6393 --save '' --appendonly no --daemonize yes
./target/release/reru --port 6394 &
redis-cli -p 6394 FLUSHALL
redis-cli -p 6393 FLUSHALL

python3 scripts/bench_vs_redis.py \
  --redis-port 6393 --reru-port 6394 \
  --redis-label "Redis 8.8.0" --reru-label "ReRu 0.1.0-alpha.1"
```

`redis-benchmark` prints `WARNING: Could not fetch server CONFIG` against ReRu,
because ReRu does not implement the configuration probe it sends. The warning is
harmless and the harness ignores it.

Run `python3 scripts/bench_vs_redis.py --help` for the request count, run count,
test list, client counts and pipeline depths.

## Reference run

| | |
| --- | --- |
| Date | 2026-11-13, 07:01 UTC |
| Host | macOS 27.0.0, arm64, Apple M5 Pro, 15 cores, 24 GiB |
| Release | ReRu 0.1.0-alpha.1 |
| Reference | Redis 8.8.0, default configuration, no persistence |
| Client | redis-benchmark 8.8.0 |
| Load | 20,000 requests per test, 3 runs, 1,000 request warmup, median reported |
| Dataset | Empty at start; the harness creates only what the tests write |

`c` is the number of connections and `P` is the pipeline depth. Ratios above
1.00 mean ReRu is faster.

### Pipeline 1 (one command in flight)

| Test | c | Redis req/s | ReRu req/s | ReRu / Redis | Redis p50 ms | ReRu p50 ms | Redis p99 ms | ReRu p99 ms |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| SET | 1 | 16935 | 62112 | 3.67x | 0.06 | 0.01 | 0.07 | 0.02 |
| SET | 8 | 86956 | 194175 | 2.23x | 0.09 | 0.03 | 0.10 | 0.08 |
| SET | 64 | 222222 | 204082 | 0.92x | 0.21 | 0.17 | 0.35 | 0.30 |
| GET | 1 | 16964 | 61162 | 3.61x | 0.06 | 0.01 | 0.07 | 0.02 |
| GET | 8 | 88496 | 194175 | 2.19x | 0.09 | 0.03 | 0.10 | 0.08 |
| GET | 64 | 222222 | 208333 | 0.94x | 0.21 | 0.17 | 0.34 | 0.28 |
| INCR | 1 | 17021 | 59524 | 3.50x | 0.06 | 0.01 | 0.07 | 0.02 |
| INCR | 8 | 88106 | 190476 | 2.16x | 0.09 | 0.03 | 0.10 | 0.08 |
| INCR | 64 | 222222 | 190476 | 0.86x | 0.21 | 0.18 | 0.35 | 0.43 |
| LPUSH | 1 | 16978 | 41754 | 2.46x | 0.06 | 0.02 | 0.07 | 0.04 |
| LPUSH | 8 | 86207 | 63092 | 0.73x | 0.09 | 0.12 | 0.11 | 0.44 |
| LPUSH | 64 | 222222 | 63291 | 0.28x | 0.22 | 0.94 | 0.35 | 4.33 |
| LPOP | 1 | 17007 | 42644 | 2.51x | 0.06 | 0.02 | 0.07 | 0.04 |
| LPOP | 8 | 86580 | 63492 | 0.73x | 0.09 | 0.12 | 0.10 | 0.43 |
| LPOP | 64 | 222222 | 63492 | 0.29x | 0.21 | 0.98 | 0.35 | 4.29 |
| SADD | 1 | 16935 | 61162 | 3.61x | 0.06 | 0.01 | 0.07 | 0.02 |
| SADD | 8 | 87719 | 198020 | 2.26x | 0.09 | 0.03 | 0.10 | 0.06 |
| SADD | 64 | 222222 | 208333 | 0.94x | 0.21 | 0.17 | 0.35 | 0.27 |
| HSET | 1 | 17079 | 60976 | 3.57x | 0.06 | 0.01 | 0.07 | 0.02 |
| HSET | 8 | 86956 | 192308 | 2.21x | 0.09 | 0.03 | 0.10 | 0.08 |
| HSET | 64 | 222222 | 194175 | 0.87x | 0.21 | 0.17 | 0.35 | 0.34 |
| ZADD | 1 | 17036 | 61920 | 3.63x | 0.06 | 0.01 | 0.07 | 0.02 |
| ZADD | 8 | 86956 | 196078 | 2.25x | 0.09 | 0.03 | 0.11 | 0.06 |
| ZADD | 64 | 222222 | 202020 | 0.91x | 0.21 | 0.17 | 0.35 | 0.29 |

### Pipeline 16 (16 commands per round trip)

| Test | c | Redis req/s | ReRu req/s | ReRu / Redis | Redis p50 ms | ReRu p50 ms | Redis p99 ms | ReRu p99 ms |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| SET | 1 | 277778 | 512820 | 1.85x | 0.06 | 0.01 | 0.07 | 0.02 |
| SET | 8 | 1052632 | 327869 | 0.31x | 0.11 | 0.05 | 0.14 | 0.16 |
| SET | 64 | 2222222 | 363636 | 0.16x | 0.38 | 0.49 | 0.53 | 1.39 |
| GET | 1 | 294118 | 465116 | 1.58x | 0.06 | 0.01 | 0.07 | 0.02 |
| GET | 8 | 1111111 | 392157 | 0.35x | 0.11 | 0.05 | 0.12 | 0.14 |
| GET | 64 | 2857143 | 408163 | 0.14x | 0.31 | 0.45 | 0.57 | 1.25 |
| INCR | 1 | 294118 | 425532 | 1.45x | 0.06 | 0.01 | 0.07 | 0.02 |
| INCR | 8 | 1052632 | 273973 | 0.26x | 0.11 | 0.06 | 0.14 | 0.18 |
| INCR | 64 | 2500000 | 246914 | 0.10x | 0.34 | 0.26 | 0.81 | 1.66 |
| LPUSH | 1 | 273973 | 156250 | 0.57x | 0.06 | 0.02 | 0.07 | 0.04 |
| LPUSH | 8 | 1000000 | 109290 | 0.11x | 0.12 | 0.13 | 0.14 | 1.30 |
| LPUSH | 64 | 2222222 | 108696 | 0.05x | 0.41 | 0.81 | 0.84 | 11.91 |
| LPOP | 1 | 266667 | 158730 | 0.60x | 0.06 | 0.02 | 0.07 | 0.04 |
| LPOP | 8 | 1000000 | 109890 | 0.11x | 0.13 | 0.13 | 0.14 | 1.37 |
| LPOP | 64 | 2222222 | 109290 | 0.05x | 0.41 | 0.94 | 0.77 | 13.44 |
| SADD | 1 | 294118 | 454546 | 1.55x | 0.06 | 0.01 | 0.07 | 0.02 |
| SADD | 8 | 1052632 | 357143 | 0.34x | 0.11 | 0.05 | 0.14 | 0.14 |
| SADD | 64 | 2500000 | 400000 | 0.16x | 0.34 | 0.44 | 0.61 | 1.33 |
| HSET | 1 | 256410 | 434783 | 1.70x | 0.06 | 0.01 | 0.08 | 0.02 |
| HSET | 8 | 1000000 | 281690 | 0.28x | 0.13 | 0.05 | 0.15 | 0.17 |
| HSET | 64 | 2222222 | 263158 | 0.12x | 0.42 | 0.28 | 0.81 | 1.53 |
| ZADD | 1 | 266667 | 444444 | 1.67x | 0.06 | 0.01 | 0.07 | 0.02 |
| ZADD | 8 | 1000000 | 312500 | 0.31x | 0.13 | 0.05 | 0.14 | 0.16 |
| ZADD | 64 | 2000000 | 333333 | 0.17x | 0.43 | 0.49 | 0.84 | 1.48 |

RSS after the run was about 5 MiB for each server, rounded down to whole MiB.
The dataset is a handful of keys, so this says nothing about memory per key or
per connection.

## How to read the numbers

The result depends on how the client talks to the server, not only on the server:

- **One connection, one command in flight.** ReRu is 3.5-3.7 times faster for
  every type measured, lists included. Part of that is the design: one thread
  per connection blocks in `read`, so there is no event-loop wakeup per command.
  Part of it is the host: a macOS `kqueue` wakeup costs more than a Linux
  `epoll` wakeup, which penalises Redis here. Do not expect this ratio on Linux.
- **One connection, pipeline 16.** ReRu stays ahead by 1.45-1.85x, and lists are
  in the same band (1.5x).
- **Eight connections, pipeline 1.** ReRu stays ahead by about 2.2x, lists
  included (2.0-2.3x).
- **Eight connections, pipeline 16.** Redis pulls ahead: about 3-3.8x for
  non-list types, 3.5-4x for lists.
- **64 connections, pipeline 1.** Roughly level for non-list types
  (0.86-0.94x); lists sit at 0.74-0.85x.
- **64 connections, pipeline 16.** Redis wins by 6-10x for non-list types and
  about 8x for lists.

The list ratios above come from
[List representation change](#list-representation-change-39). The list rows in
the reference run table were recorded before that change and are kept as
before-change data.

The shape follows the design. ReRu serialises every command through one
`command_gate` mutex and spends one thread per connection, so it cannot use extra
cores for I/O and cannot batch a pipeline as cheaply as an event loop.

Lists used to be the outlier at every concurrency level. `Value::List` was a
`Vec`, so front operations shifted every element and a push or pop cost O(n).
The list rows in the reference run above predate that change; lists now use a
`VecDeque` and reach the same band as the other types. See
[List representation change](#list-representation-change-39).

## List representation change (#39)

`LPUSH` and `LPOP` were O(n): the list was a `Vec`, so every front operation
shifted the remaining elements. At 20,000 elements they measured 0.05x of Redis
and p99 rose to 12-13 ms.

`Value::List` is now a `VecDeque`. The table below was measured back to back on
the reference host with the pre-change binary built from `main` at `c56a61d` and
the changed binary, four harness executions alternating the order, medians of
two executions per side. Redis was measured in the same windows.

| Test | c | P | Redis req/s | Before req/s | After req/s | Before / Redis | After / Redis | Improvement |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| LPUSH | 1 | 1 | 17,723 | 43,480 | 63,227 | 2.45x | 3.57x | 1.45x |
| LPUSH | 8 | 1 | 85,838 | 62,894 | 173,241 | 0.73x | 2.02x | 2.75x |
| LPUSH | 64 | 1 | 219,780 | 59,888 | 162,645 | 0.27x | 0.74x | 2.72x |
| LPUSH | 1 | 16 | 263,158 | 150,376 | 400,000 | 0.57x | 1.52x | 2.66x |
| LPUSH | 8 | 16 | 934,783 | 107,824 | 267,857 | 0.12x | 0.29x | 2.48x |
| LPUSH | 64 | 16 | 2,111,111 | 108,108 | 271,205 | 0.05x | 0.13x | 2.51x |
| LPOP | 1 | 1 | 17,978 | 41,841 | 64,135 | 2.33x | 3.57x | 1.53x |
| LPOP | 8 | 1 | 85,663 | 61,920 | 178,700 | 0.72x | 2.09x | 2.89x |
| LPOP | 64 | 1 | 217,391 | 61,280 | 177,733 | 0.28x | 0.82x | 2.90x |
| LPOP | 1 | 16 | 263,158 | 151,524 | 416,848 | 0.58x | 1.58x | 2.75x |
| LPOP | 8 | 16 | 954,545 | 108,699 | 258,938 | 0.11x | 0.27x | 2.38x |
| LPOP | 64 | 16 | 2,222,222 | 108,699 | 246,970 | 0.05x | 0.11x | 2.27x |

At 64 connections with pipeline 16, p99 for `LPUSH` fell from 11.8-11.9 ms to
1.6-1.8 ms, and for `LPOP` from 12.5-12.6 ms to 1.6-1.7 ms. At 64 connections
with pipeline 1, p99 fell from about 4.3-5.5 ms to about 0.4 ms.

The remaining gap against Redis at 64 connections is the same gap the other
types show: one mutex serialises commands and each connection owns a thread. The
list operations themselves are no longer the bottleneck.

## What this does not prove

- **Not "ReRu is faster than Redis".** It leads on one connection, and at eight
  connections with one command in flight. It loses with eight connections when
  the pipeline is deep, and is level or behind at 64 connections with either
  pipeline. Clients commonly use several connections, where the gap is widest.
- **Not "ReRu uses less memory".** The run holds almost no data. ReRu's
  `maxmemory` accounting is a logical estimate, not allocator RSS, so it cannot
  be compared with Redis's `used_memory`; this page compares process RSS only.
  Memory needs its own harness with a fixed dataset.
- **Not anything about Linux, containers, TLS, persistence, replication,
  clustering, or large keys and values.** None of those were exercised.
- **Not tuned Redis.** Redis ran with its defaults, no I/O threads and no
  allocator tuning. A tuned Redis would be faster still.
- **Not precise at the top end.** At 64 connections with pipeline 16 the Redis
  pass lasts under 10 ms, so those ratios are coarse.
- **Not stable across releases.** Numbers move with the machine, the OS, the
  load and the allocator. Re-run on the target host before making a decision.

## Reporting a comparison

If you publish a number, include the host, the OS, both server versions, the
exact harness command and the request count. A single unlabeled "requests per
second" figure is not evidence.

Run the harness yourself, with `--json` if you want to post-process the result.
