"""Compare a running Redis server with a running ReRu server (standard library only).

Both servers must already be running; this script never starts or stops them.

    redis-server --port 6393 --save '' --appendonly no --daemonize yes
    ./target/release/reru --port 6394 &
    python3 scripts/bench_vs_redis.py --redis-port 6393 --reru-port 6394

The script discards a warmup pass, repeats every configuration, alternates the
order of the two servers so drift is shared, and reports medians. Percentiles come
from `redis-benchmark`, which ships with Redis. See docs/BENCHMARKS.md for what
the numbers do and do not mean.
"""
import argparse
import csv
import io
import json
import os
import platform
import shutil
import socket
import statistics
import subprocess
import sys
from datetime import datetime, timezone

DEFAULT_TESTS = "set,get,incr,lpush,lpop,sadd,hset,zadd"
TARGETS = ("Redis", "ReRu")


def fail(message, code=1):
    print(f"error: {message}", file=sys.stderr)
    sys.exit(code)


def port_open(port):
    try:
        with socket.create_connection(("127.0.0.1", port), timeout=2):
            return True
    except OSError:
        return False


def cpu_name():
    if sys.platform == "darwin":
        result = subprocess.run(
            ["sysctl", "-n", "machdep.cpu.brand_string"],
            capture_output=True,
            text=True,
            check=False,
        )
        if result.stdout.strip():
            return result.stdout.strip()
    if sys.platform.startswith("linux"):
        try:
            with open("/proc/cpuinfo", encoding="utf-8") as handle:
                for line in handle:
                    if line.startswith("model name"):
                        return line.split(":", 1)[1].strip()
        except OSError:
            pass
    return platform.processor() or platform.machine()


def benchmark_version(benchmark):
    result = subprocess.run(
        [benchmark, "--version"], capture_output=True, text=True, check=False
    )
    text = (result.stdout or result.stderr).strip()
    return text.split()[-1] if text else "unknown"


def run_once(benchmark, port, requests, clients, pipeline, tests):
    command = [
        benchmark,
        "-h",
        "127.0.0.1",
        "-p",
        str(port),
        "-n",
        str(requests),
        "-c",
        str(clients),
        "-P",
        str(pipeline),
        "-t",
        tests,
        "--csv",
    ]
    try:
        result = subprocess.run(
            command, capture_output=True, text=True, check=False, timeout=900
        )
    except subprocess.TimeoutExpired:
        fail(f"redis-benchmark did not finish within 900 s for port {port}")
    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip() or "no output"
        fail(f"redis-benchmark failed for port {port}: {detail}")

    rows = {}
    header = None
    for row in csv.reader(io.StringIO(result.stdout)):
        if not row:
            continue
        if header is None:
            header = row
            continue
        record = dict(zip(header, row))
        try:
            rows[record["test"]] = {
                "rps": float(record["rps"]),
                "p50": float(record["p50_latency_ms"]),
                "p99": float(record["p99_latency_ms"]),
            }
        except (KeyError, ValueError):
            fail(
                "unexpected redis-benchmark CSV layout; "
                f"header was {header} and row was {row}"
            )
    if not rows:
        fail(f"redis-benchmark returned no results for port {port}")
    return rows


def listener_pid(port):
    """PID of the process listening on `port`, when `lsof` is available."""
    lsof = shutil.which("lsof")
    if not lsof:
        return None
    try:
        listeners = subprocess.run(
            [lsof, "-ti", f"tcp:{port}", "-sTCP:LISTEN"],
            capture_output=True,
            text=True,
            check=False,
        ).stdout.split()
    except OSError:
        return None
    return listeners[0] if listeners else None


def rss_kib(port):
    """Resident set size of the process listening on `port`, in KiB."""
    ps = shutil.which("ps")
    pid = listener_pid(port)
    if not ps or not pid:
        return None
    try:
        out = subprocess.run(
            [ps, "-o", "rss=", "-p", pid],
            capture_output=True,
            text=True,
            check=False,
        ).stdout.strip()
        return int(out) if out.isdigit() else None
    except (OSError, ValueError):
        return None


def median(metrics, key):
    return statistics.median(sample[key] for sample in metrics)


def parse_list(raw, convert, name):
    try:
        return [convert(item) for item in raw.split(",") if item]
    except ValueError:
        fail(f"--{name} expects comma-separated numbers, got {raw!r}")


def collect(args, benchmark):
    configs = [(clients, pipeline) for clients in args.clients for pipeline in args.pipelines]
    ports = {label: port for label, port in zip(TARGETS, (args.redis_port, args.reru_port))}
    samples = {}

    for clients, pipeline in configs:
        if args.warmup:
            for label in TARGETS:
                run_once(
                    benchmark, ports[label], args.warmup, clients, pipeline, args.tests
                )
        for run in range(args.runs):
            order = TARGETS if run % 2 == 0 else tuple(reversed(TARGETS))
            for label in order:
                rows = run_once(
                    benchmark, ports[label], args.requests, clients, pipeline, args.tests
                )
                bucket = samples.setdefault((label, clients, pipeline), {})
                for test, metrics in rows.items():
                    bucket.setdefault(test, []).append(metrics)
    return samples


def summarise(samples, args):
    table = {}
    for (label, clients, pipeline), tests in samples.items():
        key = f"c={clients} P={pipeline}"
        for test, metrics in tests.items():
            row = table.setdefault(test, {}).setdefault(key, {})
            row[label] = {
                "rps": round(median(metrics, "rps"), 1),
                "p50": round(median(metrics, "p50"), 3),
                "p99": round(median(metrics, "p99"), 3),
            }
    return table


def ratio(value, baseline):
    if not baseline:
        return "n/a"
    return f"{value / baseline:.2f}x"


def environment(args, benchmark):
    return {
        "date": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "host": f"{platform.system()} {platform.release()} {platform.machine()}",
        "cpu": cpu_name(),
        "cores": os.cpu_count(),
        "redis_benchmark": benchmark_version(benchmark),
        "redis": f"{args.redis_label} (port {args.redis_port})",
        "reru": f"{args.reru_label} (port {args.reru_port})",
        "tests": args.tests,
        "requests": args.requests,
        "warmup": args.warmup,
        "runs": args.runs,
        "rss_kib": {
            "Redis": rss_kib(args.redis_port),
            "ReRu": rss_kib(args.reru_port),
        },
    }


def print_markdown(table, args, env):
    print(f"Reference run: {env['date']} on {env['host']}, {env['cores']} cores")
    print(f"Redis: {env['redis']} - ReRu: {env['reru']}")
    basis = "single run" if args.runs == 1 else f"{env['runs']} runs, median reported"
    print(
        f"redis-benchmark {env['redis_benchmark']}, {env['requests']} requests per test, {basis}"
    )
    for label in TARGETS:
        rss = env["rss_kib"][label]
        print(f"{label} RSS after the run: {'unavailable' if rss is None else f'{rss // 1024} MiB'}")
    print()

    order = [f"c={clients} P={pipeline}" for clients in args.clients for pipeline in args.pipelines]
    print(
        "| Test | Configuration | Redis req/s | ReRu req/s | ReRu / Redis | "
        "Redis p50 ms | ReRu p50 ms | Redis p99 ms | ReRu p99 ms |"
    )
    print("| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |")
    for test in table:
        for key in order:
            row = table[test].get(key)
            if not row:
                continue
            redis = row["Redis"]
            reru = row["ReRu"]
            print(
                f"| {test.upper()} | {key} | {redis['rps']:.0f} | {reru['rps']:.0f} | "
                f"{ratio(reru['rps'], redis['rps'])} | {redis['p50']:.2f} | {reru['p50']:.2f} | "
                f"{redis['p99']:.2f} | {reru['p99']:.2f} |"
            )


def main():
    parser = argparse.ArgumentParser(
        description="Compare a running Redis server with a running ReRu server."
    )
    parser.add_argument("--redis-port", type=int, required=True, help="reference Redis port")
    parser.add_argument("--reru-port", type=int, required=True, help="ReRu port")
    parser.add_argument(
        "--redis-label", default="Redis", help="name printed for the reference server"
    )
    parser.add_argument("--reru-label", default="ReRu", help="name printed for ReRu")
    parser.add_argument(
        "--tests",
        default=DEFAULT_TESTS,
        help=(
            "redis-benchmark -t list (default "
            f"{DEFAULT_TESTS}); all tests share one process, so keep lpush and "
            "lpop together or list state grows across configurations"
        ),
    )
    parser.add_argument("--requests", type=int, default=20000, help="requests per test (default 20000)")
    parser.add_argument(
        "--warmup",
        type=int,
        default=1000,
        help="warmup requests, discarded; 0 skips the warmup pass (default 1000)",
    )
    parser.add_argument("--runs", type=int, default=3, help="measured runs per configuration (default 3)")
    parser.add_argument("--clients", default="1,8,64", help="connection counts (default 1,8,64)")
    parser.add_argument("--pipelines", default="1,16", help="pipeline depths (default 1,16)")
    parser.add_argument("--json", action="store_true", help="print JSON instead of a table")
    args = parser.parse_args()

    args.clients = parse_list(args.clients, int, "clients")
    args.pipelines = parse_list(args.pipelines, int, "pipelines")
    if not args.clients or not args.pipelines:
        fail("--clients and --pipelines need at least one value")
    if min(args.clients) < 1 or min(args.pipelines) < 1:
        fail("--clients and --pipelines must be positive")
    if args.runs < 1 or args.requests < 1 or args.warmup < 0:
        fail("--runs and --requests must be positive; --warmup cannot be negative")
    if not args.tests.strip():
        fail("--tests needs at least one test name")
    if args.redis_port == args.reru_port:
        fail("--redis-port and --reru-port must differ", 2)

    benchmark = shutil.which("redis-benchmark")
    if not benchmark:
        fail("redis-benchmark not found; it ships with Redis", 2)
    for label, port in (("Redis", args.redis_port), ("ReRu", args.reru_port)):
        if not port_open(port):
            fail(f"{label} is not accepting connections on 127.0.0.1:{port}", 2)
    pids = {label: listener_pid(port) for label, port in (("Redis", args.redis_port), ("ReRu", args.reru_port))}
    if pids["Redis"] and pids["Redis"] == pids["ReRu"]:
        fail(
            "both ports are served by the same process; "
            "start a separate Redis and ReRu server",
            2,
        )

    samples = collect(args, benchmark)
    table = summarise(samples, args)
    env = environment(args, benchmark)
    if args.json:
        raw = {}
        for (label, clients, pipeline), tests in samples.items():
            for test, metrics in tests.items():
                raw.setdefault(test, {}).setdefault(f"c={clients} P={pipeline}", {})[
                    label
                ] = metrics
        print(json.dumps({"environment": env, "results": table, "samples": raw}, indent=2))
    else:
        print_markdown(table, args, env)


if __name__ == "__main__":
    main()
