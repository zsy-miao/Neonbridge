#!/usr/bin/env python3
"""Summarize title-specific import failures from Proton +module,+loaddll logs."""
import argparse
import ast
import json
from pathlib import Path, PureWindowsPath
import re
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from project_tuxbox.inspect import digest, image, launch_target, manifest


PROVIDERS = {"d3d12_xs.dll", "xmem.dll", "dxcompiler_xs.dll", "dxcompiler.dll",
             "xg_xs.dll", "xcrdapi.dll", "acphal.dll", "mfplat.dll", "mfplat_sys.dll",
             "dstorage.dll", "dstoragecore.dll", "dxil.dll", "gameinput.dll", "mfplat_tombox.dll"}
LOAD = re.compile(r'^(?:[\d.]+:)?([\da-f]+):trace:loaddll:build_module Loaded L"([^"]+)" at ([\da-f]+): native', re.I | re.M)
LOCAL_LOAD = re.compile(r':trace:loaddll:build_module Loaded L"([^"]+)" at [\da-f]+: (?:native|builtin)', re.I)
MISSING = re.compile(r':err:module:import_dll Library (\S+) \(which is needed by L"([^"]+)"\) not found', re.I)
UNRESOLVED = re.compile(r':warn:module:import_dll No implementation for (\S+) imported from L"([^"]+)"', re.I)


def title_exception(trace, tid, title, base):
    # First-chance exceptions can be handled. Tie the debugger's terminal diagnostic
    # to its most recent SEH event on the title thread, and verify a title PC/RVA pair.
    prefix = re.compile(rf"^(?:[\d.]+:)?{re.escape(tid)}:", re.I)
    terminal = re.compile(r"wine: (Unimplemented function (\S+) called at address"
                          r"|Unhandled page fault on ([^\n]+?) at address)"
                          r" (?:0x)?([0-9a-f]+) \(thread ([0-9a-f]+)\), starting debugger\.\.\.", re.I)
    raised = re.compile(r"wine: Unhandled exception 0x00000001 in thread ([0-9a-f]+) at address "
                        r"([0-9a-f]+) \(thread ([0-9a-f]+)\), starting debugger\.\.\.", re.I)
    dispatch = re.compile(r':trace:seh:dispatch_exception code=([\da-f]+) [^\n]*?addr=([\da-f]+)', re.I)
    unwind = re.compile(r':warn:seh:virtual_unwind backtrace: ([\da-f]+): L"([^"]+)" \+ ([\da-f]+)\.', re.I)
    fatal = re.compile(r':warn:seh:OutputDebugString[AW] L?"Fatal error: [^"]*?\\n(.+?)\\n')
    event, title_rva, diagnostic = None, None, None
    for line in trace.splitlines():
        if prefix.match(line):
            if emitted := fatal.search(line):
                diagnostic = emitted[1].strip()
            elif ":dispatch_exception backtrace: --- Exception " in line:
                event, title_rva = None, None
            elif found := dispatch.search(line):
                # Unreal's crash handler emits debug strings between the completed
                # fault unwind and Wine's terminal message on this same thread.
                if title_rva is not None and found[1].lower() in ("40010006", "4001000a"):
                    continue
                event, title_rva = (found[1].lower(), int(found[2], 16)), None
            elif event and title_rva is None and (frame := unwind.search(line)):
                offset = int(frame[3], 16)
                if PureWindowsPath(frame[2]).name.casefold() == title.casefold() and int(frame[1], 16) == base + offset:
                    title_rva = hex(offset)
        elif crash := raised.fullmatch(line):
            if (diagnostic and title_rva and event == ("1", int(crash[2], 16))
                    and int(crash[1], 16) == int(crash[3], 16) == int(tid, 16)):
                return {"kind": "title-fatal-error", "exception_code": "0x00000001",
                        "title_frame_rva": title_rva, "title_diagnostic": diagnostic}
        elif crash := terminal.fullmatch(line):
            expected = "80000100" if crash[2] else "c0000005"
            if int(crash[5], 16) != int(tid, 16) or event != (expected, int(crash[4], 16)) or title_rva is None:
                continue
            failure = {"kind": "unimplemented-import" if crash[2] else "access-violation",
                       "exception_code": "0x" + expected, "title_frame_rva": title_rva}
            if crash[2]:
                failure["symbol"] = crash[2]
            else:
                failure["access"] = crash[3]
            if diagnostic:
                failure["title_diagnostic"] = diagnostic
            return failure
    raise ValueError("No terminal exception and matching title backtrace on the title loader thread")


def hashes_at(directory, names):
    files = list(directory.iterdir())
    result = {}
    for name in sorted(names):
        candidates = [f for f in files if f.name.casefold() == name and f.is_file()]
        if len(candidates) != 1:
            raise ValueError(f"Missing or ambiguous provider file for {name} in {directory}")
        result[name] = digest(candidates[0])
    return result


def graphics_device_loss(trace, tid, title):
    # vkd3d identifies the executable on its loader thread. Use that process
    # ID when looking for a device removal reported by a queue worker.
    owner = re.search(rf"^[\d.]+:([0-9a-f]+):{re.escape(tid)}:info:vkd3d-proton:"
                      rf"vkd3d_instance_apply_application_workarounds: Detected game {re.escape(title)},",
                      trace, re.I | re.M)
    if owner and re.search(rf"^[\d.]+:{owner[1]}:[0-9a-f]+:warn:vkd3d-proton:"
                           r'd3d12_device_mark_as_removed: Device [0-9a-f]+ is lost '
                           r'\(reason 0x887a0005, "VK_ERROR_DEVICE_LOST"\)', trace[owner.end():], re.I | re.M):
        return {"kind": "graphics-device-lost", "hresult": "0x887a0005",
                "vulkan_status": "VK_ERROR_DEVICE_LOST"}


def exception_thread(trace, target, loader_tid):
    # Multiple workers can fault together. WineDbg may select a different
    # worker from the one identified by Wine's terminal exception line.
    table = trace.partition("\nThreads:\n")[2]
    current_title = False
    title_threads = set()
    for line in table.splitlines():
        if process := re.fullmatch(r"[0-9a-f]+ (.+)", line, re.I):
            name = process[1]
            current_title = name.startswith("(D) ") and PureWindowsPath(name[4:]) == target
        elif current_title and (thread := re.match(r"\s+([0-9a-f]+)\s+-?\d+\b", line, re.I)):
            title_threads.add(int(thread[1], 16))
    terminal_ids = {int(tid, 16) for tid in re.findall(
        r"^wine: (?:Unhandled exception|Unhandled page fault|Unimplemented function) [^\n]+"
        r"\(thread ([0-9a-f]+)\), starting debugger\.\.\.$", trace, re.M | re.I)}
    if len(terminal_ids) == 1 and int(loader_tid, 16) in title_threads and terminal_ids <= title_threads:
        return f"{terminal_ids.pop():04x}"
    return loader_tid


def attempt(path, original, sha, code, prefix_system32=None, *, allow_timeout=False):
    trace = path.read_text(errors="replace")
    command = re.search(r"^Command: (.+)$", trace, re.M)
    version = re.search(r"^Proton: (.+)$", trace, re.M)
    if not command or not version:
        raise ValueError(f"Missing Proton trace header in {path}")
    staged = Path(ast.literal_eval(command[1])[0])
    if staged.name.casefold() != original.name.casefold() or digest(staged) != sha:
        raise ValueError(f"Executable from {path} differs from the supplied package")
    loads = LOAD.findall(trace)
    suffix = [p.casefold() for p in staged.parts]
    matches = []
    for tid, name, addr in loads:
        win = PureWindowsPath(name)
        parts = [p.casefold() for p in win.parts[1:]]
        if win.is_absolute() and parts and parts == suffix[-len(parts):]:
            matches.append((tid, win, int(addr, 16)))
    if len(matches) != 1:
        raise ValueError("Expected one native load of the staged title executable")
    tid, target, base = matches[0]
    thread_trace = "\n".join(line for line in trace.splitlines()
                             if re.match(rf"^(?:[\d.]+:)?{re.escape(tid)}:", line, re.I))
    missing = list(dict.fromkeys(m for m, name in MISSING.findall(thread_trace) if PureWindowsPath(name) == target))
    imports = [(symbol, PureWindowsPath(name)) for symbol, name in UNRESOLVED.findall(thread_trace)]
    symbols = sorted({s for s, name in imports if name == target}, key=str.casefold)
    local = {p.name.casefold() for name in LOCAL_LOAD.findall(thread_trace)
             if (p := PureWindowsPath(name)).parent == target.parent}
    providers = hashes_at(staged.parent, PROVIDERS & local)
    provider_paths = [target.parent / name for name in providers]
    result = {"provider": version[1], "app_local_provider_sha256": providers, "unix_exit": code,
              "missing_modules": missing, "unresolved_direct_symbols": symbols,
              "frame_verified": False}
    if prefix_system32 is not None:
        system32 = PureWindowsPath(r"C:\windows\system32")
        loaded = {PureWindowsPath(p).name.casefold() for thread, p, _ in loads
                  if thread == tid and PureWindowsPath(p).parent == system32}
        result["prefix_system32_provider_sha256"] = hashes_at(
            prefix_system32, loaded & {"xgameruntime.dll", "gameconfighelper.dll"})
        provider_paths.extend(system32 / name for name in result["prefix_system32_provider_sha256"])
    indirect = {}
    for path in provider_paths:
        missing_symbols = sorted({s for s, caller in imports if caller == path}, key=str.casefold)
        if missing_symbols:
            indirect[path.name] = missing_symbols
    result["unresolved_provider_symbols"] = indirect
    failures = re.findall(r':err:module:loader_init Importing dlls for L"([^"]+)" failed, status ([0-9a-f]+)',
                          thread_trace, re.I)
    statuses = [status.lower() for name, status in failures if PureWindowsPath(name) == target]
    if statuses == ["c0000135"] and code == 53 and missing:
        return {**result, "stage": "loader", "loader_status": "0xc0000135"}
    if allow_timeout and code == 124 and not statuses and not missing:
        entered_map = re.search(r':warn:seh:OutputDebugStringA ".*LogLoad: Took [\d.]+ seconds to LoadMap\((/Game/[^)]+)\)', thread_trace)
        fatal = re.search(r'^wine: (?:Unhandled|Unimplemented function)|:OutputDebugString[AW] L?"Fatal error:', trace, re.M)
        if entered_map and not fatal and not graphics_device_loss(trace, tid, target.name):
            return {**result, "stage": "title-code", "termination": "harness-timeout", "loaded_map": entered_map[1]}
    if statuses or missing or code in (124, 137, 143):
        raise ValueError("Exit or diagnostics do not establish a supported terminal outcome")
    if code != 0 and (loss := graphics_device_loss(trace, tid, target.name)):
        return {**result, "stage": "title-code", "observed_failure": loss}
    fault_tid = exception_thread(trace, target, tid)
    failure = title_exception(trace, fault_tid, target.name, base)
    if code == 0 and failure["kind"] != "title-fatal-error":
        raise ValueError("Zero launcher exit without a confirmed title fatal error")
    if fault_tid != tid:
        failure["thread_role"] = "title-worker"
    return {**result, "stage": "title-code", "observed_failure": failure}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("package", type=Path, help="original manifest directory")
    parser.add_argument("--run", action="append", required=True, nargs=3, metavar=("LABEL", "LOG", "UNIX_EXIT"),
                        help="one terminated launch trial; its unchanged staging copy must still exist")
    parser.add_argument("--prefix-system32", type=Path, metavar="PATH",
                        help="matching Wine prefix System32 directory used for the supplied trials")
    parser.add_argument("--allow-timeout", action="store_true",
                        help="record a bounded Unreal run when the title thread confirms map load")
    args = parser.parse_args()
    original = launch_target(args.package, None)
    _, identity, entries = manifest(args.package.resolve())
    pe = image(original)
    trials = {}
    for label, trace, code in args.run:
        if label in trials:
            raise ValueError(f"Duplicate trial label: {label}")
        trials[label] = attempt(Path(trace), original, pe["sha256"], int(code), args.prefix_system32,
                                allow_timeout=args.allow_timeout)
    name, app_id, family, _ = entries[0]
    direct = {d["module"].lower(): d["symbols"] for d in pe["imports"]}
    absent = {module.lower() for trial in trials.values() for module in trial["missing_modules"]}
    report = {"type": "retail-loader", "package_version": identity.attrib["Version"], "app_id": app_id,
              "family": family, "executable": name, "sha256": pe["sha256"], "entry_rva": pe["entry_rva"],
              "ordinary_modules": len(pe["imports"]), "ordinary_symbols": sum(len(d["symbols"]) for d in pe["imports"]),
              "delay_imports": {m["module"]: len(m["symbols"]) for m in pe["delay_imports"]},
              "imports_from_missing_modules": {m: direct[m] for m in sorted(absent)}, "trials": trials}
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError) as exc:
        sys.exit(f"Loader report: {exc}")
