#!/usr/bin/env python3
"""Build and run the Electra COPY list fixture on Linux using an existing dedicated Valve Proton prefix.

Supply the patched provider, its unmodified predecessor, and a workspace build directory. The guest
makes the title's compact-list and placement calls with patterned NV12 bytes and padded row pitch.
The GPU child gets a read-only host filesystem, its own temporary mount, and no network.
"""
import argparse
import json
from pathlib import Path
import shutil
import subprocess
import sys
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from build import Toolchain
from project_tuxbox.inspect import digest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("provider", "proton", "prefix", "work", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--parent-provider", type=Path, help="unmodified provider for the reference comparison")
    parser.add_argument("--tuxbox", action="store_true", help="copy a native decoder NV12 surface to TuxBox guest resources")
    args = parser.parse_args()
    if not args.tuxbox and not args.parent_provider:
        parser.error("--parent-provider is required for the reference comparison")
    work, distribution = args.work.resolve(), args.proton.resolve()
    work.mkdir(parents=True, exist_ok=True)
    host_files = distribution / "files"
    tools = Toolchain()
    with TemporaryDirectory(dir=work) as name:
        trial = Path(name)
        temp = trial / "tmp"
        runtime = trial / "runtime"
        temp.mkdir()
        runtime.mkdir(mode=0o700)
        prefix = trial / "prefix"
        # Keep the supplied baseline prefix untouched.
        subprocess.run(["cp", "-a", "--reflink=auto", str(args.prefix.resolve()), str(prefix)], check=True)
        shutil.copyfile(args.provider, trial / "d3d12_xs.dll")
        for lib, group in (("d3d12.dll", "vkd3d-proton"), ("d3d12core.dll", "vkd3d-proton"),
                           ("dxgi.dll", "dxvk")):
            shutil.copyfile(host_files / "lib/wine" / group / "x86_64-windows" / lib, trial / lib)
        obj = tools.compile(ROOT / "tools/reference/media-copy/guest.cpp", trial, "/W4", "/WX",
                            f"/I{ROOT / 'runtime/d3d12'}")
        exe = trial / "copy_probe.exe"
        tools.link(exe, obj, "d3d12.lib", "dxgi.lib", "kernel32.lib")
        command = ["bwrap", "--die-with-parent", "--unshare-pid", "--unshare-net", "--ro-bind", "/", "/", "--dev-bind", "/dev",
                   "/dev", "--proc", "/proc", "--tmpfs", "/tmp", "--bind", str(trial), str(trial),
                   "--unsetenv", "WAYLAND_DISPLAY", "--chdir", str(trial)]
        env = dict(WINEPREFIX=prefix, WINESERVER=host_files / "bin/wineserver", WINEDEBUG="-all", VKD3D_DEBUG="warn",
                   WINEDLLOVERRIDES="d3d12,d3d12core,dxgi=n,b", XDG_CACHE_HOME=trial / "cache",
                   XDG_RUNTIME_DIR=runtime, TMPDIR=temp, VKD3D_SHADER_CACHE_PATH=trial)
        for key, value in env.items():
            command.extend(["--setenv", key, str(value)])
        command.extend(["--", "xvfb-run", "-a", "timeout", "-k", "2s", "45s",
                        str(host_files / "bin/wine"), str(exe), *(["--tuxbox"] if args.tuxbox else [])])
        with (work / "probe.log").open("wb") as log:
            run = subprocess.run(command, stdout=log, stderr=subprocess.STDOUT, timeout=60)
        raw = trial / "result.txt"
        observed = dict(row.split("=", 1) for row in raw.read_text().splitlines()) if raw.exists() else {}
        if run.returncode or observed.get("plane_0_matching") != "3572" or observed.get("plane_1_matching") != "1786":
            raise ValueError(f"Copy readback did not pass: {run.returncode}, {observed}. See {work / 'probe.log'}")
        if args.tuxbox and any(observed.get(key) != "1" for key in ("foreign_guest_rejected", "stale_decoder_rejected")):
            raise ValueError("Missing foreign-resource or stale-frame rejection")
        result = dict(host=(distribution / "version").read_text().strip(), adapter_pci=observed["adapter"],
                      provider_sha256=digest(args.provider), guest_sha256=digest(exe),
                      copy_iid="30bc77b9-8379-4b25-95de-b8588c42275d",
                      scope="Standalone two-plane NV12 split using the retail method sequence, real GPU execution and readback",
                      type=3, device_create_slot=73, device_placement_slot=44,
                      compact_slots=dict(close=9, reset=10, copy_texture_region=12, resource_barrier=15),
                      desktop_slots=dict(close=9, reset=10, copy_texture_region=16, resource_barrier=26),
                      extent=[94, 38], guest_pitch=256, guest_chroma_alias_width=128, guest_chroma_alias_gpuva="0x2600",
                      luma_bytes_expected=3572, luma_bytes_matched=int(observed["plane_0_matching"]),
                      chroma_bytes_expected=1786, chroma_bytes_matched=int(observed["plane_1_matching"]),
                      caller_released_source_and_alias_before_submission=True)
        if args.tuxbox:
            result.update(source="Native ID3D12Resource created by the independent decoder-side device",
                          destinations="TuxBox guest resources", foreign_guest_rejected=True, stale_decoder_rejected=True,
                          foreign_decoder_adapter_rejected=observed.get("foreign_decoder_rejected"))
        else:
            result.update(parent_provider_sha256=digest(args.parent_provider),
                          patch_sha256=digest(ROOT / "tools/reference/tombox-media-copy.patch"))
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n")


if __name__ == "__main__":
    main()
