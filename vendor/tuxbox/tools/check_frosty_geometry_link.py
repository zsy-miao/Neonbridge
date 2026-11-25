#!/usr/bin/env python3
"""Check prepared or on-demand shader links using Frosty's actual D3D12 pipeline streams."""
import argparse
import json
from pathlib import Path
import shutil
import sys
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from build import Toolchain
from project_tuxbox.host import Host
from project_tuxbox.inspect import digest
from project_tuxbox.runtime_shaders import shader_session
from project_tuxbox.shader_prep import FROSTY_PLAN, cooked_shader, tuxbox_plan

PROGRAMS = {"vertex": ("global", 2423), "geometry": ("global", 2420),
            "pixel0": ("global", 2421), "pixel1": ("material", 687)}


def guest_check(host, exe, arguments, expected):
    receipt = exe.with_name("result.txt")
    receipt.unlink(missing_ok=True)
    result = host.run(exe, arguments, timeout=90, capture=True)
    checks = dict(line.split("=", 1) for line in receipt.read_text().splitlines()) if receipt.exists() else {}
    if result.returncode or checks != expected:
        sys.stderr.buffer.write(result.stdout)
        raise ValueError(f"Geometry pipeline control failed ({result.returncode}, {checks})")
    return checks


def runtime_checks(trial, host, exe, adapter, programs):
    # The guest redirects its temporary directory here before loading any shaders.
    cache = trial / "temp/tuxbox_shader_cache"
    host.env.pop("TUXBOX_SHADER_SESSION", None)
    expected = dict(pci=adapter.lower(), geometry="0", vertex="0", wrong_pixel="0")
    offline = guest_check(host, exe, [adapter, "offline"], expected)
    keys = []
    for producer, pixel in (("geometry", "pixel0"), ("vertex7", "pixel7")):
        source, consumer = (programs[stage]["dxbc_sha256"] for stage in (producer, pixel))
        order = digest(trial / "by_source" / (consumer + ".cso"))
        keys.append(f"{source}_{consumer}_{order}")
    expected.update(geometry="1", vertex="1")
    runs = {"offline": {"guest": offline}}
    cold_replies = {}
    for mode in ("cold", "warm"):
        events = []
        with shader_session(cache, host.env, events.append):
            session = cache / host.env["TUXBOX_SHADER_SESSION"]
            checks = guest_check(host, exe, [adapter, "dynamic"], expected)
            replies = {p.stem: digest(p) for p in session.glob("*.cso")}
            if set(replies) != set(keys) or any(session.glob("*.failed")):
                raise ValueError(f"Incomplete {mode} runtime replies: {replies}")
        expected_events = {f"Compiled Xbox shader {key}" for key in keys} if mode == "cold" else set()
        if len(events) != len(expected_events) or set(events) != expected_events:
            raise ValueError(f"Unexpected {mode} compiler activity: {events}")
        if mode == "cold":
            cold_replies = replies
        elif replies != cold_replies:
            raise ValueError("Warm replies differ from the cold links")
        runs[mode] = {"guest": checks, "host_compiles": len(events), "replies": len(replies)}
    return dict(runs=runs, compiled_links=cold_replies)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("pak", "cache", "provider", "prefix", "proton", "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--adapter", required=True)
    material = parser.add_mutually_exclusive_group()
    material.add_argument("--frame-seven", action="store_true", help="also check the five-target material vertex/pixel pipeline")
    material.add_argument("--bridge", action="store_true", help="also check the four-target bridge finale material")
    parser.add_argument("--runtime-links", action="store_true", help="test on-demand geometry and vertex links cold and warm")
    args = parser.parse_args()
    plan, _ = tuxbox_plan(FROSTY_PLAN, json.loads(FROSTY_PLAN.read_text()), True)
    rows = {(row["archive"], row["entry"]): row for row in plan["shaders"]}
    programs = {name: rows[key] for name, key in PROGRAMS.items()}
    if args.runtime_links:
        programs.pop("pixel1")
        programs["pixel0"] = rows["material", 700]
    check_material = args.frame_seven or args.bridge or args.runtime_links
    if check_material:
        vs, ps = (31059, 31420) if args.bridge else (2517, 2522)
        programs.update(vertex7=rows["material", vs], pixel7=rows["material", ps])
    parent = ROOT / "build/frosty-geometry-link"
    parent.mkdir(parents=True, exist_ok=True)
    with TemporaryDirectory(dir=parent) as temporary:
        trial = Path(temporary)
        if args.bridge:
            (trial / "bridge.txt").touch()
        staging = trial / "by_source"
        staging.mkdir()
        for name, row in programs.items():
            (trial / f"{name}.dxbc").write_bytes(cooked_shader(args.pak, row))
            key = row["dxbc_sha256"] + ".cso"
            shutil.copyfile(args.cache / "by_source" / key, staging / key)
        if not args.runtime_links:
            shutil.copyfile(staging / (programs["vertex"]["dxbc_sha256"] + ".cso"), trial / "wrong-stage.cso")
            links = []
            for n in range(2):
                pair = programs["geometry"]["dxbc_sha256"] + "_" + programs[f"pixel{n}"]["dxbc_sha256"]
                source = args.cache / "by_gs_ps" / (pair + ".cso")
                links.append(dict(pair=pair, compiled_sha256=digest(source)))
                shutil.copyfile(source, trial / f"linked{n}.cso")
            if check_material:
                vs, ps = (programs[name]["dxbc_sha256"] for name in ("vertex7", "pixel7"))
                pair = f"{vs}_{ps}"
                source = args.cache / "by_vs_ps" / (pair + ".cso")
                links.append(dict(pair=pair, compiled_sha256=digest(source)))
                shutil.copyfile(source, trial / "linked7.cso")
                shutil.copyfile(staging / (ps + ".cso"), trial / "wrong-vertex-stage.cso")
                (trial / "vertex7.key").write_text(pair + ".cso")
                (trial / "other-consumer.key").write_text(f"{vs}_{programs['pixel0']['dxbc_sha256']}.cso")
        t = Toolchain()
        source = ROOT / "tools/reference/chase-link/geometry.cpp"
        obj = t.compile(source, trial, "/W4", "/WX", f"/I{ROOT / 'runtime/d3d12'}")
        exe = trial / "geometry.exe"
        t.link(exe, obj, str(ROOT / "build/d3d12-smoke/d3d12_xs.lib"), "dxgi.lib", "kernel32.lib")
        shutil.copyfile(args.provider, trial / "d3d12_xs.dll")
        host = Host(None, args.proton, args.prefix)
        output = dict(proton=host.version, provider_sha256=digest(args.provider), guest_sha256=digest(exe),
                      source_shaders={name: row["dxbc_sha256"] for name, row in programs.items()})
        if args.bridge:
            for stage in ("vertex", "pixel"):
                source_key = output["source_shaders"].pop(stage + "7")
                output["source_shaders"]["bridge_" + stage] = source_key
            output["bridge_portable_pixel_sha256"] = digest(staging / (programs["pixel7"]["dxbc_sha256"] + ".cso"))
        if args.runtime_links:
            output.update(runtime_checks(trial, host, exe, args.adapter, programs))
        else:
            expected = dict(pci=args.adapter.lower(), linked="2", absent="1", wrong_consumer="1", wrong_stage="1")
            if check_material:
                expected.update(vertex_linked="1", vertex_absent="1", vertex_wrong_consumer="1", vertex_wrong_stage="1")
            if args.bridge:
                expected["bridge_pixel_absent"] = "1"
            output.update(links=links, checks=guest_check(host, exe, [args.adapter], expected))
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(output, indent=2) + "\n")


if __name__ == "__main__":
    main()
