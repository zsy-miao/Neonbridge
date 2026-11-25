#!/usr/bin/env python3
"""Build and stage Frosty from the original package into a dedicated Linux working directory."""
import argparse
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import sys
from tempfile import NamedTemporaryFile
from zipfile import ZipFile

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from build import OUTPUT, Toolchain, build_target, find_dxc
from project_tuxbox.host import Host, proton_installation
from project_tuxbox.inspect import digest
from project_tuxbox.media import build_frosty_media, register_frosty_media
from project_tuxbox.shader_prep import FROSTY_PLAN, prepare_for_launch

BIN = Path("Frosty/Binaries/XSX")
TITLE = "Frosty-XSX-Shipping.exe"
SHIPPED = {TITLE.lower(), "gamechat2.dll", "msvcp140.dll", "vcruntime140.dll", "vcruntime140_1.dll"}
PROVIDERS = {"d3d12": "d3d12-smoke/d3d12_xs.dll", "dxc-utils": "dxc-utils/dxcompiler_xs.dll",
             "gameinput": "gameinput/GameInput.dll", "xmem": "xmem-smoke/xmem.dll",
             "acphal": "acphal/AcpHal.dll", "xcrdapi": "xcrdapi/xcrdapi.dll",
             "xgameruntime": "xgameruntime/xgameruntime.dll"}
POINTER = OUTPUT / "frosty-native/latest-independent"
ENGINE_INI = Path("Frosty/Config/UserEngine.ini")
XG_PATH = Path("Mount") / BIN / "xg_xs.dll"


def publish(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    with NamedTemporaryFile(dir=path.parent, delete=False) as tmp:
        tmp.write(data.encode())
    Path(tmp.name).replace(path)


def verify_staged_files(work):
    stage = work / "Mount"
    if Path((work / "stage").read_text().strip()) != stage:
        raise ValueError("The package location differs from the prepared working directory")
    files = json.loads((work / "providers.json").read_text())
    marker = work / "inputs.json"
    if marker.exists():
        mode = json.loads(marker.read_text()).get("xg_provider", "packaged")
        expected_origin = {"packaged": "Packaged Xbox XG", "tuxbox": "TuxBox xg"}.get(mode)
        if expected_origin is None or files.get(str(XG_PATH), {}).get("origin") != expected_origin:
            raise ValueError("The staged XG provider does not match the preparation selection")
    original = {Path(name).name.lower() for name, info in files.items() if info["origin"] == "Original retail package"}
    if original != SHIPPED:
        raise ValueError("Reprepare the working copy to record all five original retail binaries")
    for relative, record in files.items():
        if digest(work / relative) != record["sha256"]:
            raise ValueError(f"Staged file differs from the preparation record: {relative}")
    expected = SHIPPED | {Path(name).name.lower() for name in files if Path(name).parent == Path("Mount") / BIN}
    binaries = {p.name.lower() for p in (stage / BIN).iterdir() if p.suffix.lower() in {".dll", ".exe"}}
    if binaries != expected:
        raise ValueError(f"Unexpected retail binary set: {sorted(binaries ^ expected)}")


def stage_xg_provider(tools, packaged_os, work, guest_bin, *, use_tuxbox):
    if use_tuxbox:
        build_target(tools, "xg", None)
        source = OUTPUT / "xg-provider/xg_xs.dll"
        origin = "TuxBox xg"
    else:
        source = packaged_os / "Windows/System32/xg_xs.dll"
        origin = "Packaged Xbox XG"
    expected = digest(source)
    dest = guest_bin / "xg_xs.dll"
    manifest = work / "providers.json"
    if manifest.is_file():
        previous = json.loads(manifest.read_text()).get(str(XG_PATH))
        if not previous or previous.get("origin") != origin or previous.get("sha256") != expected:
            raise ValueError("The XG provider changed; prepare a new working directory")
        if not dest.is_file() or digest(dest) != previous["sha256"]:
            raise ValueError("The staged XG provider differs from its preparation record")
    shutil.copyfile(source, dest)
    if digest(dest) != expected:
        raise ValueError("The staged XG provider does not match its source")
    return {"origin": origin, "sha256": expected}


def refresh_xg(work):
    """Explicitly replace a verified TuxBox XG build in an existing work directory."""
    import fcntl

    with (work / "launch.lock").open("a+b") as stream:
        fcntl.flock(stream, fcntl.LOCK_EX | fcntl.LOCK_NB)
        if json.loads((work / "inputs.json").read_text()).get("xg_provider") != "tuxbox":
            raise ValueError("XG refresh requires an existing TuxBox XG working directory")
        verify_staged_files(work)
        tools = Toolchain()
        build_target(tools, "xg", None)
        source = OUTPUT / "xg-provider/xg_xs.dll"
        expected = digest(source)
        destination = work / XG_PATH
        temp = None
        try:
            with NamedTemporaryFile(dir=destination.parent, delete=False) as out, source.open("rb") as src:
                temp = Path(out.name)
                shutil.copyfileobj(src, out)
            if digest(temp) != expected:
                raise ValueError("The refreshed XG provider does not match its source")
            files = json.loads((work / "providers.json").read_text())
            files[str(XG_PATH)] = {"origin": "TuxBox xg", "sha256": expected}
            temp.replace(destination)
            publish(work / "providers.json", json.dumps(files, indent=2) + "\n")
        finally:
            if temp is not None:
                temp.unlink(missing_ok=True)
    print(f"Refreshed TuxBox XG in {work}; sha256 {expected}")


def update_provider_manifest(work, files):
    manifest = work / "providers.json"
    # Older preparations installed this optional helper in the prefix.
    legacy = "prefix/drive_c/windows/system32/gameconfighelper.dll"
    if manifest.is_file():
        previous = json.loads(manifest.read_text()).get(legacy, {})
        path = work / legacy
        if previous.get("origin") == "TuxBox gameconfig" and path.is_file() and digest(path) == previous.get("sha256"):
            path.unlink()
    publish(manifest, json.dumps(files, indent=2) + "\n")


def prepare(source, packaged_os, work, proton, windows_dxc, *, select=True, use_tuxbox_xg=False):
    import fcntl

    plan = json.loads(FROSTY_PLAN.read_text())
    if source == work or source in work.parents or work in source.parents:
        raise ValueError("Choose a dedicated working directory outside the original package")
    source_bin = source / BIN
    if digest(source_bin / TITLE) != plan["executable_sha256"]:
        raise ValueError("The executable differs from the verified Frosty shader profile")
    binaries = {p.name.lower() for p in source_bin.iterdir() if p.suffix.lower() in {".dll", ".exe"}}
    if binaries != SHIPPED:
        raise ValueError(f"Expected the original retail binaries. Differences: {sorted(binaries ^ SHIPPED)}")
    work.mkdir(parents=True, exist_ok=True)
    stage, prefix = work / "Mount", work / "prefix"
    settings = {"source": str(source), "packaged_os": str(packaged_os), "proton": str(proton)}
    if use_tuxbox_xg:
        settings["xg_provider"] = "tuxbox"
    marker = work / "inputs.json"
    lock = work / "launch.lock"
    with lock.open("a+b") as stream:
        fcntl.flock(stream, fcntl.LOCK_EX | fcntl.LOCK_NB)
        if marker.is_file():
            if json.loads(marker.read_text()) != settings:
                raise ValueError("This directory belongs to a different package, Proton installation, or XG selection")
        elif any(path != lock for path in work.iterdir()):
            raise ValueError("Use an empty directory or an existing independent Frosty setup")
        else:
            publish(marker, json.dumps(settings, indent=2) + "\n")
        if not (work / "package-copied").exists():
            subprocess.run(["cp", "-a", "--reflink=auto", "--", str(source) + "/.", stage], check=True)
            (work / "package-copied").touch()
        tools = Toolchain()
        guest_bin = stage / BIN
        xg_record = stage_xg_provider(tools, packaged_os, work, guest_bin, use_tuxbox=use_tuxbox_xg)
        for target in PROVIDERS:
            build_target(tools, target, find_dxc(None) if target == "d3d12" else None)
        host = Host("proton", proton, prefix)
        host.env.setdefault("UMU_RUNTIME_UPDATE", "0")
        host.run(OUTPUT / "xgameruntime/runtime-control.exe", [], timeout=120).check_returncode()
        drive = prefix / "dosdevices/g:"
        if drive.is_symlink():
            if drive.resolve() != stage:
                raise ValueError(f"This prefix maps G: to {drive.resolve()}")
        else:
            drive.symlink_to(stage, target_is_directory=True)
        system = prefix / "drive_c/windows/system32"
        files = {}

        def record(path, origin):
            files[str(path.relative_to(work))] = {"origin": origin, "sha256": digest(path)}

        for original in source_bin.iterdir():
            if original.name.lower() in SHIPPED:
                dest = guest_bin / original.name
                if not dest.exists() or digest(dest) != digest(original):
                    shutil.copyfile(original, dest)
                record(dest, "Original retail package")
        http = guest_bin / "tuxbox_xhttp.dll"
        shutil.copyfile(OUTPUT / "xmem-smoke" / http.name, http)
        record(http, "TuxBox xmem")
        for target, relative in PROVIDERS.items():
            folder = system if target == "xgameruntime" else guest_bin
            name = Path(relative).name
            if target == "acphal":
                name = name.lower()
            dest = folder / name
            shutil.copyfile(OUTPUT / relative, dest)
            record(dest, f"TuxBox {target}")
        files[str(XG_PATH)] = xg_record
        for name in ("dxcompiler.dll", "dxil.dll"):
            dest = guest_bin / name
            if windows_dxc.is_dir():
                shutil.copyfile(windows_dxc / name, dest)
            else:
                with ZipFile(windows_dxc) as release, release.open(f"bin/x64/{name}") as src, dest.open("wb") as dst:
                    shutil.copyfileobj(src, dst)
            record(dest, "Microsoft DirectX Shader Compiler for Windows")
        for name in build_frosty_media(guest_bin, packaged_os, proton):
            origin = {"mfplat_sys.dll": "Proton", "mfhevcdec.dll": "Packaged Xbox HEVC",
                      "frosty_packaged_aac.dll": "Packaged Xbox AAC",
                      "frosty_packaged_h264.dll": "Packaged Xbox H.264"}.get(name, "TuxBox media")
            record(guest_bin / name, origin)
        register_frosty_media(guest_bin, proton, prefix)
        update_provider_manifest(work, files)
        if os.environ.get("TUXBOX_SHADER_PREP") != "chase":
            os.environ["TUXBOX_SHADER_PREP"] = "menu"
        if not prepare_for_launch(guest_bin / TITLE, host):
            raise ValueError("The TuxBox graphics provider did not match the Frosty shader profile")
        publish(work / "stage", str(stage) + "\n")
        publish(work / "proton", str(proton) + "\n")
        if select:
            publish(POINTER, str(work) + "\n")
    print(f"Frosty ready in {stage}. Launch with tools/run_frosty.sh --desktop --work {shlex.quote(str(work))}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source", type=Path, nargs="?", help="original Frosty Mount directory")
    checks = parser.add_mutually_exclusive_group()
    checks.add_argument("--check", action="store_true", help="verify the staged binaries and check for old graphics overrides")
    checks.add_argument("--refresh-xg", action="store_true", help="rebuild and explicitly refresh XG in a verified TuxBox-XG working directory")
    # The launcher already holds launch.lock during this check.
    checks.add_argument("--check-launch", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--packaged-os", type=Path, help="default: EmbeddedXvd beside Mount")
    parser.add_argument("--work", type=Path, required=True, help="dedicated directory, preferably on the game's filesystem")
    parser.add_argument("--windows-dxc", type=Path, help="Microsoft DXC Windows release zip or bin/x64 directory")
    parser.add_argument("--proton", type=Path, help="installed Proton distribution")
    parser.add_argument("--no-select", action="store_true", help="keep the current default working directory")
    parser.add_argument("--tuxbox-xg", action="store_true", help="build and stage TuxBox XG in a new working directory")
    args = parser.parse_args()
    try:
        if args.check or args.check_launch:
            verify_staged_files(args.work.resolve())
            return
        if args.refresh_xg:
            refresh_xg(args.work.resolve())
            return
        if not args.source or not args.windows_dxc:
            parser.error("preparation requires the original Mount directory and --windows-dxc")
        source = args.source.resolve()
        prepare(source, (args.packaged_os or source.parent / "EmbeddedXvd").resolve(), args.work.resolve(),
                proton_installation(args.proton or os.getenv("PROTONPATH")), args.windows_dxc.resolve(),
                select=not args.no_select, use_tuxbox_xg=args.tuxbox_xg)
    except (OSError, RuntimeError, ValueError, subprocess.SubprocessError) as exc:
        parser.exit(1, f"Frosty preparation failed: {exc}\n")


if __name__ == "__main__":
    main()
