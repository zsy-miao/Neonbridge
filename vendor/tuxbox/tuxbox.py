#!/usr/bin/env python3
"""Project TuxBox development CLI."""
import argparse
from contextlib import redirect_stderr
import json
import os
from pathlib import Path
import subprocess
import sys
from time import monotonic
from xml.etree import ElementTree

try:
    import pefile
except ImportError:
    sys.exit("Install the inspection dependency with python -m pip install -r requirements.txt")

from project_tuxbox.backend_log import check_log_path
from project_tuxbox.game_log import game_log
from project_tuxbox.graphics_profile import PROFILES, graphics_profile
from project_tuxbox.host import Host
from project_tuxbox.inspect import binary_file, image, launch_target, package, package_root
from project_tuxbox.d3d12_status import d3d12_table
from project_tuxbox.progress_map import library_table, shader_table, write_map
from project_tuxbox.shader_prep import prepare_for_launch
from project_tuxbox.check import run_checks
from project_tuxbox.symbols import lookup_symbols
from project_tuxbox.verify import SUITES, verify, status


def launch_game(args):
    exe = launch_target(args.target.expanduser(), args.app_id)
    package_dir = package_root(args.target.expanduser(), exe)
    host = Host(args.backend, args.proton, args.prefix, offline=args.offline)
    if not (args.graphics_menu or args.graphics_profile):
        return run_game(args, exe, host, package_dir)
    if args.prepare_only:
        raise ValueError("Graphics profiles apply to a game launch")
    with graphics_profile(exe, host, args.graphics_profile, menu=args.graphics_menu) as profile:
        if profile is None:
            return 0
        return run_game(args, exe, host, package_dir, profile)


def run_game(args, exe, host, package_dir, profile=None):
    if args.log_file:
        check_log_path(host.backend, host.env, exe.parent, args.log_file.expanduser().resolve())
    with game_log(exe, args.log_file) as log:
        log.status(f"{exe.name} on {host.backend} ({host.version})")
        log.status(f"Executable: {exe}")
        if profile:
            log.status(f"Graphics profile: {profile}")
        if host.backend in ("wine", "proton"):
            log.status(f"Prefix: {host.env['WINEPREFIX']}")
        if host.offline:
            log.status("Network: offline. The game cannot reach servers or upload crash reports.")
        try:
            # Project Scarlett's recorded archive blocks launch for many minutes.
            # The runtime worker still compiles requests. --prepare-only keeps the archive pass.
            scarlett = exe.name.casefold() == "game_scarlett_master.exe"
            automatic = (not args.no_shader_prep and os.environ.get("TUXBOX_SHADER_PREP") != "0"
                         and "-nullrhi" not in (arg.lower() for arg in args.arguments)
                         and not scarlett)
            if args.prepare_only or automatic:
                try:
                    with redirect_stderr(log):
                        supported = prepare_for_launch(exe, host)
                except (OSError, ValueError, subprocess.SubprocessError) as exc:
                    if args.prepare_only:
                        raise
                    log.status(f"Shader preparation incomplete: {exc}. Starting the game.")
                else:
                    if args.prepare_only:
                        log.status("Shader preparation complete." if supported else
                                   "No matching shader preparation profile for this title and provider.")
                        return 0 if supported else 1
            log.status("Starting game")
            start = monotonic()
            code = host.run_game(exe, args.arguments, log, package=package_dir,
                                 mangohud=args.mangohud, profile_pipelines=args.profile_pipelines).returncode
        except KeyboardInterrupt:
            log.status("Launch interrupted")
            return 130
        result = f"exit code {code}" if code >= 0 else f"signal {-code}"
        log.status(f"Game exited with {result} after {monotonic() - start:.1f}s")
        return code if code >= 0 else 128 - code


def misplaced_option(arguments, options):
    """First launcher option found among the game arguments, or None."""
    return next((arg for arg in arguments if arg.split("=", 1)[0] in options), None)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    subs = parser.add_subparsers(required=True, dest="command")
    inspect = subs.add_parser("inspect", help="inspect a local package, PE image or .bin file")
    inspect.add_argument("path", type=Path)
    inspect.add_argument("--json", action="store_true", help="include full symbol tables and file hashes")
    inspect.add_argument("--rva", action="append", type=lambda value: int(value, 0), metavar="ADDRESS",
                         help="look up a PE-relative address in an Unreal Symbols .bin (repeatable)")
    run = subs.add_parser("run", help="launch a prepared x64 executable or manifest entry")
    run.add_argument("--app-id", help="select a manifest entry by its Id")
    run.add_argument("--log-file", type=Path, metavar="PATH", help="append the live game log to this file")
    run.add_argument("--mangohud", action="store_true",
                     help="show Vulkan performance metrics and record frame times beside the game log on Linux")
    run.add_argument("--profile-pipelines", action="store_true",
                     help="record vkd3d-proton pipeline creation times and keys beside the game log")
    graphics = run.add_mutually_exclusive_group()
    graphics.add_argument("--graphics-menu", action="store_true", help="choose a Project Scarlett graphics profile")
    graphics.add_argument("--graphics-profile", choices=PROFILES, help="select a Project Scarlett graphics profile")
    run.add_argument("--offline", action="store_true",
                     help="on Linux, launch without network access so the game cannot upload crash reports")
    preparation = run.add_mutually_exclusive_group()
    preparation.add_argument("--prepare-only", action="store_true", help="prepare supported shader caches and exit")
    preparation.add_argument("--no-shader-prep", action="store_true", help="skip shader preparation for this launch")
    run.add_argument("target", type=Path)
    run.add_argument("arguments", nargs=argparse.REMAINDER)
    check = subs.add_parser("verify", help="run the built homebrew fixtures and write an evidence record")
    check.add_argument("--benchmark-samples", type=int, choices=range(1, 31), default=1, metavar="1..30")
    check.add_argument("--output", required=True, type=Path)
    check.add_argument("--only", choices=SUITES, action="append", help="restrict checks to the given suite (repeatable)")
    check.add_argument("--adapter", help="select a hardware graphics adapter by PCI vendor:device")
    gate = subs.add_parser("check", help="run every check a change needs before it can be merged")
    gate.add_argument("--no-gpu", action="store_true",
                      help="run only the checks CI runs; the change still needs a GPU run before merging")
    gate.add_argument("--baseline", type=Path, metavar="RESULTS",
                      help="compare the Lanes benchmark with an earlier check's results")
    gate.add_argument("--windows-dxc", type=Path, metavar="DIR",
                      help="directory holding Microsoft's x64 dxcompiler.dll and dxil.dll, "
                           "for the Linux D3D12 regression controls")
    gate.add_argument("--adapter", help="select a hardware graphics adapter by PCI vendor:device")
    for cmd in (run, check, gate):
        cmd.add_argument("--backend", choices=("native", "proton", "wine"))
        cmd.add_argument("--proton", help="installed Valve or GE-Proton directory")
        cmd.add_argument("--prefix", type=Path, help="Linux prefix directory for this provider")
    show = subs.add_parser("status", help="show recorded milestone evidence by host")
    status_format = show.add_mutually_exclusive_group()
    status_format.add_argument("--svg", type=Path, metavar="PATH", help="write the visual progress map")
    status_format.add_argument("--libraries", action="store_true", help="show the library behavior Frosty uses and what supplies it")
    status_format.add_argument("--d3d12-xs", action="store_true", help="show checked Xbox D3D12 interface slots")
    status_format.add_argument("--shaders", action="store_true", help="show packaged and requested shaders by stage")
    args = parser.parse_args()
    if args.command == "check":
        options = []
        for name in ("adapter", "backend", "proton", "prefix"):
            if getattr(args, name) is not None:
                options += [f"--{name}", str(getattr(args, name))]
        dxc = args.windows_dxc.expanduser().resolve() if args.windows_dxc else None
        return run_checks(gpu=not args.no_gpu, host_options=options, baseline=args.baseline, windows_dxc=dxc)
    if args.command == "status":
        evidence = Path(__file__).resolve().parent / "evidence"
        if args.svg:
            write_map(evidence, args.svg)
        elif args.libraries:
            print(library_table(evidence))
        elif args.d3d12_xs:
            print(d3d12_table(evidence))
        elif args.shaders:
            print(shader_table(evidence))
        else:
            print(status())
        return 0
    if args.command == "inspect":
        path = args.path.expanduser().resolve()
        if args.rva and (path.is_dir() or path.suffix.casefold() != ".bin"):
            parser.error("--rva needs an Unreal symbol database .bin file")
        if path.is_dir():
            data = package(path)
            if not args.json:
                print(f"Package {data['identity'].get('Name', '(unnamed)')} {data['identity'].get('Version', '')}")
                for entry in data["executables"]:
                    present = "present" if entry["present"] else "missing"
                    print(f"{entry['id']}  {entry['target']}  {entry['path']}  {present}")
                print(f"{len(data['libraries'])} DLLs inventoried; {sum('error' in lib for lib in data['libraries'])} unreadable")
                for binary in data["binary_files"]:
                    print(f"{binary['path']}  {binary.get('error') or binary['kind']}")
                print("Files by extension:", json.dumps(data["files_by_extension"]))
                return 0
        elif path.suffix.casefold() == ".bin":
            data = lookup_symbols(path, args.rva) if args.rva else binary_file(path)
            unresolved = any(item["status"] != "matched" for item in data.get("lookups", ()))
            if not args.json:
                print(f"{path.name}  {data['kind']}  {data['size_bytes']} bytes")
                for key, value in data.items():
                    if key not in ("kind", "size_bytes", "lookups"):
                        print(f"{key}  {json.dumps(value, ensure_ascii=False)}")
                for row in data.get("lookups", ()):
                    if row["status"] == "matched":
                        label = row["function"]
                        label = label if label.isprintable() else ascii(label)
                        print(f"{row['rva']}  {label} + {row['offset']}  (start {row['function_rva']})")
                    else:
                        print(f"{row['rva']}  {row['status']}")
            else:
                print(json.dumps(data, indent=2))
            return int("error" in data or unresolved)
        else:
            data = image(path)
            if not args.json:
                print(f"{data['filename']}  {data['kind']}  machine {data['machine']}  entry {data['entry_rva']}")
                for kind in ("imports", "delay_imports"):
                    for desc in data[kind]:
                        print(f"{kind}  {desc['module']}  {len(desc['symbols'])} symbols")
                return 0
        print(json.dumps(data, indent=2))
        return 1 if "error" in data else 0
    if args.command == "run":
        # Everything after the target is passed to the game, including launcher options.
        options = {string for action in run._actions for string in action.option_strings} - {"-h", "--help"}
        misplaced = misplaced_option(args.arguments, options)
        if misplaced:
            run.error(f"{misplaced} must come before the game path; after it, it would be passed to the game")
        return launch_game(args)
    host = Host(args.backend, args.proton, args.prefix)
    return verify(host, args.output, args.adapter, args.only, args.benchmark_samples)


if __name__ == "__main__":
    try:
        sys.exit(main())
    except (OSError, ValueError, pefile.PEFormatError, ElementTree.ParseError, subprocess.SubprocessError) as exc:
        sys.exit(f"tuxbox: {exc}")
