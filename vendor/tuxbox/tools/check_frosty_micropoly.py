#!/usr/bin/env python3
"""Run Frosty's software rasterizer with packed triangles and verify every visibility texel."""
import argparse
import json
from pathlib import Path
import shutil
import sys
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from build import Toolchain
from check_frosty_instance_cull import f32, load, save
from check_frosty_shaders import GLOBAL_SHA, containers, decompress, global_archive
from frosty_pipeline_checks import check_packaged_pipelines
from project_tuxbox.host import Host
from project_tuxbox.inspect import digest
from project_tuxbox.runtime_shaders import shader_session
from project_tuxbox.shader_inventory import decoder
from project_tuxbox.shader_prep import compiler_tools

ENTRY = 3246
FINGERPRINT = "1e7aa6eec3deeeb634429870c7d8bfce63591c0e8c0bd28b08e3af0e76b865e1"
WIDTH, HEIGHT = 31, 23
RECT = (2, 2, 29, 21)
XY = ((-4, -4), (4, -4), (0, 4))
SMALL = ((-1, -1), (1, -1), (0, 1))


def pack(words, start, value, bits):
    for j in range(bits):
        words[(start + j) // 32] |= ((value >> j) & 1) << ((start + j) % 32)


def page(precision, triangles, empty):
    # Two cluster headers use structure-of-arrays columns. Cluster 1 is empty.
    words = [0] * 320
    words[0] = 2
    vertex_count = max(first + 3 for _, first, _ in triangles)
    primitive_count = max(index + 1 for index, _, _ in triangles)
    words[4] = vertex_count | (128 << 9)
    words[5] = (0 if empty else primitive_count) | (512 << 8)
    words[12:16] = [0xfffffffc, 0xfffffffc, 0, 8 | ((precision + 8) << 4) | (4 << 9) | (4 << 14)]
    for index, first, vertices in triangles:
        for i, (x, y) in enumerate(vertices):
            pack(words, 128 * 8 + (first + i) * 8, (x + 4) | ((y + 4) << 4), 8)
        # Unused records reference vertex zero three times.
        pack(words, 512 * 8 + index * 18, first | (1 << 8) | (2 << 13), 18)
    return words


def prepare(path, *, precision=3, primitive=0, empty=False, flip=False, cluster_list=((0, 0),),
            x_offset=0, protected=False, mixed=False, gradient=False):
    path.mkdir()
    groups = len(cluster_list)
    cb = [0] * 64
    cb[20] = 2  # float4 elements between instance-data columns
    cb[25] = 32 if flip else 0
    save(path / "b0", cb)
    scene = [0] * 24
    for i in range(2):
        scene[8 + 4 * i:12 + 4 * i] = [0x80008000, 0x4000, 0x80018001, 0x008e8001]
        scene[16 + 4 * i] = f32(x_offset)
    scene[22] = f32(-.25)  # instance one is farther away
    save(path / "t0", scene)
    triangles = [(primitive, 192 if primitive else 0, XY)]
    if mixed:
        triangles += [(33, 3, SMALL), (96, 192, SMALL)]
    save(path / "t1", page(precision, triangles, empty))
    save(path / "t2", [w for cluster, instance in cluster_list for w in (instance << 15, cluster << 23)])
    rect = (2, 2, 18, 18) if gradient else RECT
    left, top, right, bottom = rect
    width, height = right - left, bottom - top
    view = [0] * 200
    view[48] = view[53] = view[58] = view[63] = f32(1)
    view[62] = f32(.75)
    if gradient:
        view[50] = f32(.25)  # clip-space Z += local X / 4
    view[160:166] = [*rect, f32(width), f32(height)]
    save(path / "t3", view)
    seed = []
    for i in range(WIDTH * HEIGHT):
        # Give each texel its own initial primitive ID and seed protected pixels
        # above or at the triangle depth to exercise both words of unsigned max.
        if protected and i % 5 == 0:
            seed.append((f32(.875) << 32) | i)
        elif protected and i % 5 == 1:
            seed.append((f32(.75) << 32) | 0x4000)
        else:
            seed.append((f32(.25) << 32) | i)
    save(path / "u0", [w for v in seed for w in (v & 0xffffffff, v >> 32)])
    expected = seed.copy()
    # Coordinates are expressed in 1/256 pixel units, like the packaged rasterizer.
    scale = 1 << precision
    for primitive, _, vertices in triangles:
        points = [(round((left + (1 + x_offset + x / scale) * width / 2) * 256),
                   round((top + (1 - y / scale) * height / 2) * 256)) for x, y in vertices]
        for group, (cluster, instance) in enumerate(cluster_list):
            if empty or flip or cluster:
                continue
            depth = .75 if instance == 0 else .5
            fragment_id = ((group + 1) << 7) | primitive
            for y in range(top, bottom):
                for x in range(left, right):
                    px, py = x * 256 + 128, y * 256 + 128
                    edges = [(b[0] - a[0]) * (py - a[1]) - (b[1] - a[1]) * (px - a[0])
                             for a, b in zip(points, points[1:] + points[:1])]
                    if max(edges) < 0:
                        k = y * WIDTH + x
                        local_x = 2 * (x + .5 - left) / width - 1
                        z = depth + (local_x * .25 if gradient else 0)
                        expected[k] = max(expected[k], (f32(z) << 32) | fragment_id)
    return groups, expected


def verify(path, expected):
    words = load(path / "out0")
    observed = [lo | (hi << 32) for lo, hi in zip(words[::2], words[1::2])]
    if len(observed) != WIDTH * HEIGHT or len(words) != 2 * len(observed):
        raise ValueError(f"{path.name}: incomplete visibility readback")
    if observed != expected:
        diffs = [(k % WIDTH, k // WIDTH, f"{a:016x}", f"{b:016x}")
                 for k, (a, b) in enumerate(zip(expected, observed)) if a != b]
        raise ValueError(f"{path.name}: {len(diffs)} pixel differences (x, y, expected, GPU): {diffs[:8]}")
    seed = load(path / "u0")
    changed = [v for i, v in enumerate(observed) if v != (seed[2 * i] | (seed[2 * i + 1] << 32))]
    return dict(checked_texels=len(observed), changed_texels=len(changed),
                fragment_words=[f"{v:016x}" for v in sorted(set(changed))], readback_sha256=digest(path / "out0"))


def run(mount, host, adapter, provider, trial, all_pipelines=False):
    archive, entries = global_archive(mount)
    ((_, shader, _),) = containers(decompress(archive, entries[ENTRY], decoder()))
    source = trial / "micropoly.dxbc"
    source.write_bytes(shader)
    if digest(source) != FINGERPRINT:
        raise ValueError("Unexpected packaged MicropolyRasterize shader")
    specs = {
        "empty": dict(empty=True),
        "projected": {},
        "short-span": dict(precision=5),
        "viewport": dict(precision=0),
        "depth-plane": dict(gradient=True),
        "backface": dict(flip=True),
        "offscreen": dict(x_offset=4),
        "upper-lanes": dict(primitive=96),
        "mixed-wave-spans": dict(mixed=True),
        "second-group": dict(cluster_list=[(1, 0), (0, 0)]),
        "depth-wins": dict(cluster_list=[(0, 0), (0, 1)]),
        "equal-depth": dict(cluster_list=[(0, 0), (0, 0)]),
        "protected": dict(protected=True),
    }
    cases = {name: prepare(trial / name, **options) for name, options in specs.items()}
    (trial / "image.txt").write_text(f"{WIDTH} {HEIGHT} 2\n")
    (trial / "cases.txt").write_text("".join(f"{name} {groups}\n" for name, (groups, _) in cases.items()))
    build = Toolchain()
    obj = build.compile(ROOT / "tools/reference/micropoly/guest.cpp", trial, "/W4", "/WX", f"/I{ROOT / 'runtime/d3d12'}")
    exe = trial / "micropoly.exe"
    build.link(exe, obj, str(ROOT / "build/d3d12-smoke/d3d12_xs.lib"), "dxgi.lib", "kernel32.lib")
    shutil.copyfile(provider, trial / "d3d12_xs.dll")
    compiled = []
    cache = trial / "temp/tuxbox_shader_cache"
    with shader_session(cache, host.env, compiled.append):
        session = cache / host.env["TUXBOX_SHADER_SESSION"]
        result = host.run(exe, ["--adapter", adapter, "--report", "result.txt"], capture=True, timeout=150)
        receipt = trial / "result.txt"
        line = receipt.read_text().strip() if receipt.exists() else ""
        if result.returncode or line != f"micropoly PASS pci={adapter} cases={len(cases)}":
            sys.stderr.buffer.write(result.stdout[-4000:])
            raise ValueError(f"MicropolyRasterize dispatch failed: {line}, compilation: {compiled}")
        if compiled != [f"Compiled Xbox shader {FINGERPRINT}"]:
            raise ValueError(f"Unexpected runtime compilation: {compiled}")
        dxil = digest(session / (FINGERPRINT + ".cso"))
    report = dict(name="MicropolyRasterize", entry=ENTRY, adapter=adapter, host=host.version,
                global_archive_sha256=GLOBAL_SHA, source_sha256=FINGERPRINT, dxil_sha256=dxil,
                guest_sha256=digest(exe), provider_sha256=digest(provider), compiler_sha256=digest(compiler_tools()[0]),
                cases={name: verify(trial / name, expected) for name, (_, expected) in cases.items()})
    if all_pipelines:
        census = json.loads((ROOT / "evidence/matrix-awakens/micropoly-retail.json").read_text())
        if census["global_archive_sha256"] != GLOBAL_SHA:
            raise ValueError("MicropolyRasterize census belongs to a different archive")
        report["pipelines"] = check_packaged_pipelines(archive, entries, decoder(), host, adapter, trial, exe,
                                                      census["family"]["archive_entries"], {"cs_6_0", "cs_6_6"}, "micropoly")
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mount", type=Path)
    parser.add_argument("--proton", type=Path)
    parser.add_argument("--prefix", type=Path)
    parser.add_argument("--adapter", required=True)
    parser.add_argument("--all-pipelines", action="store_true", help="also create every packaged micropolygon compute pipeline")
    parser.add_argument("--provider", type=Path, default=ROOT / "build/d3d12-smoke/d3d12_xs.dll")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    with TemporaryDirectory(prefix="micropoly-", dir=ROOT / "build") as tmp:
        result = run(args.mount, Host("proton", args.proton, args.prefix), args.adapter.lower(), args.provider, Path(tmp), args.all_pipelines)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n")


if __name__ == "__main__":
    main()
