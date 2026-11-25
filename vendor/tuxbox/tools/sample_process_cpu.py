#!/usr/bin/env python3
"""Sample a running Linux game's process and thread CPU usage independently of graphics presents."""
import argparse
import json
import os
from pathlib import Path
import time


def stat(path):
    raw = path.read_bytes()
    end = raw.rindex(b")")
    name = raw[raw.index(b"(") + 1:end].decode(errors="replace")
    fields = raw[end + 2:].split()  # task state is field 3 in proc_pid_stat(5)
    return name, int(fields[19]), int(fields[11]) + int(fields[12])


def snapshot(proc):
    _, birth, ticks = stat(proc / "stat")
    threads = {}
    for task in (proc / "task").iterdir():
        try:
            name, start, cpu = stat(task / "stat")
        except (FileNotFoundError, ProcessLookupError):
            continue
        threads[(int(task.name), start)] = name, cpu
    with Path("/proc/stat").open() as stream:
        fields = [int(v) for v in stream.readline().split()[1:9]]
    # Linux includes guest CPU time in user/nice. Idle and iowait are not busy time.
    return (birth, ticks, threads, sum(fields), fields[3] + fields[4],
            time.monotonic(), time.time_ns())


def sample(pid, seconds, interval, output):
    proc = Path("/proc") / str(pid)
    frequency = os.sysconf("SC_CLK_TCK")
    previous = snapshot(proc)
    finish = previous[5] + seconds
    while previous[5] < finish:
        time.sleep(max(0, min(previous[5] + interval, finish) - time.monotonic()))
        try:
            current = snapshot(proc)
        except (FileNotFoundError, ProcessLookupError):
            break
        birth, cpu, threads, total, idle, end, utc_ns = current
        if birth != previous[0]:
            break  # The kernel reused the PID.
        elapsed = end - previous[5]
        scale = 100 / (frequency * elapsed)
        active = []
        for (tid, start), (name, ticks) in threads.items():
            old = previous[2].get((tid, start))
            if old and ticks > old[1]:
                active.append({"tid": tid, "name": name, "cpu_ticks": ticks - old[1]})
        host_ticks = total - previous[3]
        row = {
            "pid": pid, "clock_ticks_per_second": frequency, "end_unix_ns": utc_ns,
            "seconds": round(elapsed, 6), "process_one_core_percent": round((cpu - previous[1]) * scale, 2),
            "host_busy_percent": (round(100 * (1 - (idle - previous[4]) / host_ticks), 2)
                                  if host_ticks > 0 else None),
            "threads": sorted(active, key=lambda thread: thread["cpu_ticks"], reverse=True),
        }
        output.write(json.dumps(row, separators=(",", ":")) + "\n")
        output.flush()
        previous = current


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("pid", type=int)
    parser.add_argument("--seconds", type=float, default=15)
    parser.add_argument("--interval", type=float, default=1,
                        help="wall-clock seconds between samples (default 1)")
    parser.add_argument("--output", required=True, type=Path, help="write one JSON object per interval")
    args = parser.parse_args()
    if not 0 < args.seconds < float("inf") or not 0 < args.interval < float("inf"):
        parser.error("seconds and interval must be positive finite numbers")
    try:
        with args.output.open("x") as stream:
            sample(args.pid, args.seconds, args.interval, stream)
    except OSError as error:
        parser.exit(1, f"CPU sampling failed: {error}\n")


if __name__ == "__main__":
    main()
