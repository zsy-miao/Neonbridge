#!/usr/bin/env python3
"""Check Frosty's material classification from visibility texels to draw arguments and tile masks."""
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
from project_tuxbox.host import Host
from project_tuxbox.inspect import digest
from project_tuxbox.runtime_shaders import shader_session
from project_tuxbox.shader_inventory import decoder
from project_tuxbox.shader_prep import compiler_tools

ENTRY = 2725
FINGERPRINT = "624cea6b8644e17fe48084b21d7586e4860805b090d349db172a068ae5e0e495"
# Material 8195 lives in the second half of the shader's 512-DWORD LDS mask.
MATERIAL_IDS = ((1, 33, 8195), (12, 35, 8189))
TILE_PITCH = 37
MASK_WORDS = 2


def prepare(path, samples, *, size=(64, 64), groups=(1, 1), origin=(0, 0), slow=False, full=False, paged=False):
    path.mkdir()
    width, height = size
    gx, gy = groups
    cb = [0] * 64
    cb[9] = int(paged)
    cb[12:14] = origin
    cb[16:18] = [width - 1, height - 1]
    cb[19] = MASK_WORDS
    cb[21:23] = [257, TILE_PITCH]
    save(path / "b0", cb)
    save(path / "t0", [0] * (38 * 4 * 2))
    save(path / "t1", [0, 0, 0, 0, 1, 0, 0, 0])
    page = [0] * 160
    page[0] = 1
    # A packed cluster selects local material slots 1, 2, 3 over three
    # triangle ranges: [0, 2), [2, 4), [4, 128). Its alternate encoding
    # stores the same three intervals in a separate range table.
    if slow:
        page[27] = 0xfe000000 | (2 << 19) | 96
        page[96:99] = [(1 << 16) | (2 << 8), (2 << 16) | (2 << 8) | 2, (3 << 16) | (124 << 8) | 4]
    else:
        page[27] = 1 | (2 << 6) | (3 << 12) | (1 << 18) | (2 << 25)
    if paged:
        # All pages above the first root page use 32 KiB. Cluster 1 in
        # streaming page 2 has a different local material table from page 0.
        second_page = (128 + 32) * 1024 // 4
        page.extend([0] * (second_page + 128 - len(page)))
        page[second_page] = 2
        page[second_page + 4 + 4 + 2 * 20 + 3] = 7 | (8 << 6) | (9 << 12) | (1 << 18) | (2 << 25)
    save(path / "t2", page)
    # Two visible-cluster records select two scene primitives in one resident page.
    save(path / "t3", [1, 0, (1 << 15) | 1, ((2 << 7) | (1 << 23)) if paged else 0])
    table = [0] * (64 * 2)
    for instance, ids in enumerate(MATERIAL_IDS):
        slot = 64 * instance + (7 if paged and instance == 1 else 1)
        table[slot:slot + 3] = ids
    save(path / "t5", table)
    fragments = [(0, 0) if full else None] * (width * height)
    for x, y, instance, triangle in samples:
        fragments[y * width + x] = (instance, triangle)
    pixels = []
    for i, fragment in enumerate(fragments):
        pixels += [0 if fragment is None else ((fragment[0] + 1) << 7) | fragment[1], f32(.25 if i % 3 else .75)]
    save(path / "t4", pixels)
    counts = [0xa5c3e749] * (8200 * 5 + 8)
    masks = [0x40000000] * (8200 * MASK_WORDS + 8)
    save(path / "u0", counts)
    save(path / "u1", masks)
    expected_tiles = {}
    for group_y in range(gy):
        for group_x in range(gx):
            materials = set()
            for y in range(origin[1] + 64 * group_y, origin[1] + 64 * (group_y + 1)):
                for x in range(origin[0] + 64 * group_x, origin[0] + 64 * (group_x + 1)):
                    value = fragments[min(y, height - 1) * width + min(x, width - 1)]
                    if value is not None:
                        instance, triangle = value
                        materials.add(MATERIAL_IDS[instance][0 if triangle < 2 else 1 if triangle < 4 else 2])
            tile = group_y * TILE_PITCH + group_x
            for material in materials:
                counts[5 * material + 1] += 1
                masks[MASK_WORDS * material + tile // 32] |= 1 << (tile % 32)
            expected_tiles[tile] = sorted(materials)
    return gx, gy, width, height, (counts, masks, expected_tiles)


def fixtures(trial):
    mixed = [(0, 0, 0, 0), (3, 2, 0, 1), (15, 1, 0, 2), (15, 3, 0, 4), (0, 4, 1, 127),
             (15, 15, 1, 2), (32, 33, 1, 1), (47, 62, 0, 4), (63, 63, 1, 0)]
    specs = {
        "empty": ([], {}),
        "uniform": ([], dict(full=True)),
        "upper-lane": ([(15, 3, 0, 4)], {}),
        "mixed": (mixed, {}),
        "range-table": (mixed, dict(slow=True)),
        "resident-pages": (mixed, dict(paged=True)),
        "four-tiles": (mixed + [(64, 64, 1, 2), (66, 60, 0, 4), (1, 66, 1, 0), (66, 66, 0, 0)],
                      dict(size=(67, 67), groups=(2, 2))),
        "offset": ([(0, 0, 1, 0), (6, 3, 0, 4), (32, 33, 1, 2)], dict(origin=(5, 2))),
    }
    return {name: prepare(trial / name, samples, **options) for name, (samples, options) in specs.items()}


def verify(path, expected):
    counts, masks, tiles = expected
    for i, wanted in enumerate((counts, masks)):
        actual = load(path / f"out{i}")
        if actual != wanted:
            diffs = [(k, hex(a), hex(b)) for k, (a, b) in enumerate(zip(wanted, actual)) if a != b]
            raise ValueError(f"{path.name} u{i} word count {len(actual)}/{len(wanted)}, mismatches {diffs[:8]}")
    return dict(materials_by_tile=tiles, verified_words=len(counts) + len(masks),
                draw_arguments_sha256=digest(path / "out0"), tile_masks_sha256=digest(path / "out1"))


def run(mount, host, adapter, provider, trial):
    archive, entries = global_archive(mount)
    ((_, shader, _),) = containers(decompress(archive, entries[ENTRY], decoder()))
    source = trial / "classify.dxbc"
    source.write_bytes(shader)
    if digest(source) != FINGERPRINT:
        raise ValueError("Unexpected packaged ClassifyMaterials shader")
    cases = fixtures(trial)
    (trial / "cases.txt").write_text("".join(f"{name} {gx} {gy} {width} {height}\n"
                                   for name, (gx, gy, width, height, _) in cases.items()))
    build = Toolchain()
    obj = build.compile(ROOT / "tools/reference/classify-materials/guest.cpp", trial, "/W4", "/WX", f"/I{ROOT / 'runtime/d3d12'}")
    exe = trial / "classify.exe"
    build.link(exe, obj, str(ROOT / "build/d3d12-smoke/d3d12_xs.lib"), "dxgi.lib", "kernel32.lib")
    shutil.copyfile(provider, trial / "d3d12_xs.dll")
    compiled = []
    cache = trial / "temp/tuxbox_shader_cache"
    with shader_session(cache, host.env, compiled.append):
        session = cache / host.env["TUXBOX_SHADER_SESSION"]
        result = host.run(exe, ["--adapter", adapter, "--report", "result.txt"], capture=True, timeout=150)
        receipt = trial / "result.txt"
        line = receipt.read_text().strip() if receipt.exists() else ""
        if result.returncode or line != f"classify-materials PASS pci={adapter} cases={len(cases)}":
            sys.stderr.buffer.write(result.stdout[-4000:])
            raise ValueError(f"ClassifyMaterials dispatch failed: {line}, compilation: {compiled}")
        if compiled != [f"Compiled Xbox shader {FINGERPRINT}"]:
            raise ValueError(f"Unexpected runtime compilation: {compiled}")
        dxil = digest(session / (FINGERPRINT + ".cso"))
    report = dict(name="ClassifyMaterials", entry=ENTRY, adapter=adapter, host=host.version,
                global_archive_sha256=GLOBAL_SHA, source_sha256=FINGERPRINT, dxil_sha256=dxil,
                guest_sha256=digest(exe), provider_sha256=digest(provider), compiler_sha256=digest(compiler_tools()[0]),
                cases={name: verify(trial / name, expected) for name, (*_, expected) in cases.items()})
    return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mount", type=Path)
    parser.add_argument("--proton", type=Path)
    parser.add_argument("--prefix", type=Path)
    parser.add_argument("--adapter", required=True)
    parser.add_argument("--provider", type=Path, default=ROOT / "build/d3d12-smoke/d3d12_xs.dll")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    with TemporaryDirectory(prefix="classify-materials-", dir=ROOT / "build") as tmp:
        result = run(args.mount, Host("proton", args.proton, args.prefix), args.adapter.lower(), args.provider, Path(tmp))
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n")


if __name__ == "__main__":
    main()
