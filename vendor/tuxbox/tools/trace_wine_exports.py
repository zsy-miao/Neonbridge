#!/usr/bin/env python3
"""Count calls to up to four native DLL exports in a Wine process on x86-64 Linux.

Uses per-thread user-mode hardware counters. No debugger stops or code patches.
Start before launch. Coverage begins when the DLL is mapped and each thread is
armed; calls on short-lived threads between polls may be missed.
"""
import argparse
import ctypes
import json
import math
import os
import platform
import struct
import sys
import time
from pathlib import Path

import pefile


def emit(event, **fields):
    print(json.dumps({"time_ns": time.time_ns(), "event": event, **fields}), flush=True)


def exports(module, names):
    with pefile.PE(str(module), fast_load=True) as pe:
        if pe.FILE_HEADER.Machine != 0x8664:
            raise ValueError("the DLL must be x86-64")
        pe.parse_data_directories(directories=[pefile.DIRECTORY_ENTRY["IMAGE_DIRECTORY_ENTRY_EXPORT"]])
        table = getattr(pe, "DIRECTORY_ENTRY_EXPORT", None)
        symbols = {s.name: s for s in table.symbols if s.name} if table else {}
        result = {}
        for name in names:
            sym = symbols.get(name.encode("ascii"))
            section = pe.get_section_by_rva(sym.address) if sym else None
            if not section or sym.forwarder or not section.Characteristics & 0x20000000:
                raise ValueError(f"{name} does not name executable code in {module}")
            result[name] = sym.address
        if len(set(result.values())) != len(names):
            raise ValueError("choose distinct export addresses")
        return result


def find_process(name):
    matches = []
    for process in Path("/proc").glob("[0-9]*"):
        try:
            cmd = (process / "cmdline").read_bytes().split(b"\0", 1)[0]
            if cmd.replace(b"\\", b"/").rsplit(b"/", 1)[-1].lower() == os.fsencode(name.lower()):
                matches.append(process)
        except (FileNotFoundError, PermissionError, ProcessLookupError):
            continue
    if len(matches) > 1:
        raise ValueError(f"multiple processes named {name}; select one with --pid")
    return next(iter(matches), None)


def addresses(process, module_stat, rvas):
    base = None
    executable = []
    for row in (process / "maps").read_text().splitlines():
        span, perms, offset, device, inode, *paths = row.split(maxsplit=5)
        start, end = (int(n, 16) for n in span.split("-"))
        if "x" in perms:
            executable.append((start, end))
        if int(offset, 16) or int(inode) != module_stat.st_ino:
            continue
        major, minor = (int(n, 16) for n in device.split(":"))
        matches = os.makedev(major, minor) == module_stat.st_dev
        if not matches and paths and paths[0].startswith("/"):
            # Btrfs reports the superblock device in maps and a subvolume
            # device in stat. Compare the named inode in Wine's mount namespace.
            try:
                matches = os.path.samestat((process / "root" / paths[0][1:]).stat(), module_stat)
            except (FileNotFoundError, PermissionError, ProcessLookupError):
                pass
        if matches:
            if base is not None:
                raise ValueError("DLL has multiple header mappings")
            base = start
    if base is None:
        return None
    result = {name: base + rva for name, rva in rvas.items()}
    if all(any(start <= addr < end for start, end in executable) for addr in result.values()):
        return result
    return None


def open_counter(tid, address):
    # perf_event_attr v0. PERF_TYPE_BREAKPOINT, HW_BREAKPOINT_X, sizeof(long).
    # The kernel requires an eight-byte bp_len for an x86-64 execute breakpoint.
    attr = ctypes.create_string_buffer(72)
    struct.pack_into("=II", attr, 0, 5, len(attr))
    struct.pack_into("=Q", attr, 32, 3)  # PERF_FORMAT_TOTAL_TIME_{ENABLED,RUNNING}
    struct.pack_into("=Q", attr, 40, (1 << 2) | (1 << 5) | (1 << 6))  # pinned, exclude kernel/HV
    struct.pack_into("=IQQ", attr, 52, 4, address, 8)
    libc = ctypes.CDLL(None, use_errno=True)
    fd = libc.syscall(298, ctypes.byref(attr), tid, -1, -1, 8)  # x86-64 perf_event_open, CLOEXEC
    if fd < 0:
        error = ctypes.get_errno()
        raise OSError(error, os.strerror(error))
    return fd


def trace(args):
    if sys.platform != "linux" or platform.machine() != "x86_64":
        raise ValueError("x86-64 Linux is required")
    rvas = exports(args.module, args.exports)
    module_stat = args.module.stat()
    process = Path("/proc") / str(args.pid) if args.pid else None
    deadline = time.monotonic() + args.seconds
    counters = {}
    totals = dict.fromkeys(rvas, 0)
    last = totals.copy()
    armed = 0
    announced = False
    armed_threads = set()
    resolved = None
    # A numeric PID remains ours only while its proc directory has the original inode.
    inode = None
    try:
        while time.monotonic() < deadline:
            if process is None:
                process = find_process(args.process)
            if process is not None:
                try:
                    stat = process.stat()
                    if inode is None:
                        inode = stat.st_ino
                        emit("process", pid=int(process.name))
                    if stat.st_ino != inode:
                        break
                    current = addresses(process, module_stat, rvas)
                except (FileNotFoundError, ProcessLookupError):
                    break
                if current and resolved is None:
                    resolved = current
                    emit("mapped", module=str(args.module), exports={k: hex(v) for k, v in current.items()})
                if resolved is not None:
                    if current != resolved:
                        break
                    tids = {int(t.name) for t in (process / "task").iterdir()}
                    for tid in sorted(tids - armed_threads):
                        try:
                            for name, address in resolved.items():
                                counters[tid, name] = (open_counter(tid, address), 0)
                        except ProcessLookupError:
                            tids.discard(tid)
                            continue
                        armed_threads.add(tid)
                        armed += 1
                    if armed and not announced:
                        emit("armed", pid=int(process.name), thread_count=len(armed_threads))
                        announced = True
                    for key, (fd, previous) in list(counters.items()):
                        data = os.read(fd, 24)
                        if len(data) != 24:
                            raise ValueError(f"hardware execution counter could not run for thread {key[0]}")
                        count, enabled, running = struct.unpack("=3Q", data)
                        if enabled and not running and key[0] not in tids:
                            emit("unscheduled_thread", tid=key[0], export=key[1])
                        totals[key[1]] += count - previous
                        if key[0] in tids:
                            counters[key] = (fd, count)
                        else:
                            os.close(fd)
                            del counters[key]
                    armed_threads.intersection_update(tids)
                    if totals != last:
                        emit("calls", totals=totals)
                        last = totals.copy()
            time.sleep(0.04 if resolved else 0.02)
    finally:
        for (_, name), (fd, previous) in counters.items():
            try:
                data = os.read(fd, 24)
                if len(data) == 24:
                    totals[name] += struct.unpack("=3Q", data)[0] - previous
            finally:
                os.close(fd)
        emit("end", armed_threads=armed, totals=totals)
    if not armed:
        raise ValueError("no thread was armed; check the process name and the exact mapped DLL path")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument("--process", help="exact Windows executable basename, including .exe")
    source.add_argument("--pid", type=int, help="Linux PID")
    parser.add_argument("--module", type=Path, required=True, help="exact DLL file mapped by Wine")
    parser.add_argument("--seconds", type=float, default=180, help="maximum wall time including waiting for launch")
    parser.add_argument("exports", nargs="+")
    args = parser.parse_args()
    if not (1 <= len(args.exports) <= 4 and math.isfinite(args.seconds) and args.seconds > 0):
        parser.error("choose one to four exports and a positive finite duration")
    try:
        trace(args)
    except (OSError, ValueError, pefile.PEFormatError) as exc:
        parser.exit(1, f"{exc}\n")


if __name__ == "__main__":
    main()
