#!/usr/bin/env python3
"""Build Frosty's Media Foundation reset bridge and verify its graphics-device association.

Supply the packaged system image, the Proton distribution and a dedicated prefix.
--video also compares a decoded NV12 frame with FFmpeg. Stage mfplat.dll and
mfplat_sys.dll from --work beside the title executable.
"""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from build import Toolchain
from project_tuxbox.host import Host
from project_tuxbox.inspect import digest
from project_tuxbox.media import build_bridge


def build(work, packaged_os, proton):
    build_bridge(work, packaged_os, proton)
    source = ROOT / "tools/reference/media"
    tools = Toolchain()
    for name, extra_libs in (("sample", ("d3d11.lib",)), ("read_video", ("mfreadwrite.lib",))):
        obj = tools.compile(source / f"{name}.cpp", work, "/W4", "/WX", f"/I{ROOT / 'runtime/d3d12'}")
        tools.link(work / f"{name}.exe", obj, "mfplat.lib", "mfuuid.lib", "ole32.lib", "d3d12.lib", "kernel32.lib",
                   *extra_libs)


def invoke(host, exe, args, report):
    report.unlink(missing_ok=True)
    run = host.run(exe, args, capture=True)
    observation = {"exit": run.returncode}
    if report.exists():
        observation.update(line.split("=", 1) for line in report.read_text().splitlines())
    if run.returncode:
        sys.stderr.buffer.write(run.stdout[-2000:])
    return observation


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--packaged-os", required=True, type=Path, help="path to the packaged EmbeddedXvd directory")
    parser.add_argument("--proton", required=True, type=Path)
    parser.add_argument("--prefix", required=True, type=Path)
    parser.add_argument("--work", required=True, type=Path, help="dedicated output directory for the bridge and guests")
    parser.add_argument("--provider", type=Path, help="also check the Xbox device from this d3d12_xs.dll")
    parser.add_argument("--video", action="append", type=Path, default=[])
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    work = args.work.resolve()
    build(work, args.packaged_os, args.proton)
    host = Host("proton", args.proton, args.prefix)
    # Cover the full launcher process group when a media provider stalls during teardown.
    host.command = ["timeout", "--kill-after=3s", "25s", *host.command]
    sample = invoke(host, work / "sample.exe", [], work / "sample.txt")
    result = {"host": host.version, "dll_sha256": {name: digest(work / name) for name in ("mfplat.dll", "mfplat_sys.dll")},
              "sample": sample, "videos": []}
    success = not sample["exit"] and sample.get("verified_pixel_words") == "4"
    if args.provider:
        staged = work / "d3d12_xs.dll"
        if args.provider.resolve() != staged:
            shutil.copyfile(args.provider, staged)
        wrapped = invoke(host, work / "sample.exe", ["--xbox"], work / "sample.txt")
        result.update(provider_sha256=digest(staged), xbox_device=wrapped)
        success &= (not wrapped["exit"] and wrapped.get("verified_pixel_words") == "4"
                    and wrapped.get("wrong_token_hr") == "0x80070057")
    for video in args.video:
        oracle = subprocess.run(["ffmpeg", "-v", "error", "-i", str(video), "-frames:v", "1", "-pix_fmt", "nv12",
                                 "-f", "rawvideo", "pipe:1"], stdout=subprocess.PIPE, check=True).stdout
        with tempfile.TemporaryDirectory(dir=work) as directory:
            raw = Path(directory) / "first.nv12"
            report = Path(directory) / "read.txt"
            win_path = "Z:" + str(video.resolve()).replace("/", "\\")
            read = invoke(host, work / "read_video.exe",
                          [win_path, str(report.relative_to(work)), str(raw.relative_to(work))], report)
            actual = raw.read_bytes() if raw.exists() else b""
        matched = bool(oracle) and oracle == actual and read.get("timestamp") == "0"
        result["videos"].append({"name": video.name, **read, "oracle_sha256": hashlib.sha256(oracle).hexdigest(),
                                  "frame_sha256": hashlib.sha256(actual).hexdigest() if actual else None,
                                  "matched": matched})
        success &= not read["exit"] and matched
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    return 0 if success else 1


if __name__ == "__main__":
    sys.exit(main())
