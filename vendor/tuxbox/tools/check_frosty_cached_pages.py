#!/usr/bin/env python3
"""Compare Frosty's cached virtual-shadow-page mappings and five statistics counters with a CPU model."""
import argparse
from array import array
import json
from pathlib import Path
import subprocess
from tempfile import TemporaryDirectory

from check_frosty_shaders import ROOT, global_archive, decompress, containers, digest
from build import Toolchain, find_dxc
from project_tuxbox.host import Host
from project_tuxbox.shader_inventory import decoder

ENTRY = 1228
STRIDE = 21845
THREADS = 512
GUARD = 0xfeedbabe


def words(n, value=0):
    return array("I", [value]) * n


def workload(work, name, separate, use_history, previous):
    # The guest spaces virtual shadow maps 21,845 pages apart, independent of
    # the dispatch width. Layer zero may reuse its previous layer, layer one
    # is unmapped. Most of each layer stays outside this test's two X groups.
    virtual = 2 * STRIDE
    t = [array("I", [previous, 0xffffffff]), words(virtual), words(virtual),
         words(2 * THREADS), words(virtual), words(virtual)]
    b0, b1 = words(64), words(64)
    b0[26], b1[36], b1[39] = int(use_history), int(separate), 5
    for i in range(THREADS):
        physical = (59 * i + 13) % THREADS
        xy = (physical % 32) | ((physical // 32) << 10)
        t[1][i] = 1 | (8 if i % 13 == 0 else 0) | (i % 4) << 1 | (i << 12)
        if i % 11 == 3:
            t[1][i] &= ~1
        t[2][i] = xy | 0x40000000
        t[3][2 * physical] = 0 if i % 7 == 5 else 1
        t[3][2 * physical + 1] = 1000 + i
        t[4][i] = (i % 4) << 1
        for layer in range(2):
            t[5][layer * STRIDE + i] = 0 if i % 19 == 0 else 7
    u = [words(16, GUARD), words(virtual + 4, GUARD), words(virtual + 4, GUARD),
         words(2 * THREADS + 8, GUARD)]
    for k in range(5):
        u[0][k] = 11 + 7 * k
    for key, values in [(f"t{i}", v) for i, v in enumerate(t)] + [("b0", b0), ("b1", b1)] + [
            (f"u{i}", v) for i, v in enumerate(u)]:
        (work / f"{name}.{key}").write_bytes(values.tobytes())
    for layer in range(2):
        for i in range(THREADS):
            current = layer * STRIDE + i
            u[1][current] = u[2][current] = 0
            if not t[5][current] or t[0][layer] == 0xffffffff:
                continue
            page = t[0][layer] * STRIDE + i
            flags = t[1][page]
            if not flags & 1 or (flags ^ t[5][current]) & 8:
                continue
            xy = t[2][page] & 0xfffff
            physical = (xy & 1023) + 32 * (xy >> 10)
            if not t[3][2 * physical] & 1:
                continue
            dirty = t[4][page] if use_history else 0
            reusable = not (dirty & 4) if separate else dirty == 0
            clean = not (dirty & 2) if separate else dirty == 0
            u[0][4] += int(reusable and not clean)
            u[0][2] += int(not reusable)
            if reusable:
                u[0][0] += 1
                u[0][1] += 1
                u[0][3] += int(clean)
                u[2][current] = xy | 0x08000000
                u[1][current] = (flags & ~6) | (0 if clean else 2)
                u[3][2 * physical] = u[1][current]
                u[3][2 * physical + 1] = 1 + t[3][2 * physical + 1] if clean else 0
    return u


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mount", type=Path)
    parser.add_argument("--xshader", required=True, type=Path)
    parser.add_argument("--prefix", type=Path)
    parser.add_argument("--proton", type=Path)
    parser.add_argument("--output", required=True, type=Path)
    a = parser.parse_args()
    data, entries = global_archive(a.mount)
    native = next(containers(decompress(data, entries[ENTRY], decoder())))[1]
    host = Host(None, a.proton, a.prefix)
    base = ROOT / "build/frosty-retail-settings/compute-election"
    base.mkdir(parents=True, exist_ok=True)
    with TemporaryDirectory(dir=base) as tmp:
        work = Path(tmp)
        (work / "1228.dxbc").write_bytes(native)
        subprocess.run([a.xshader.resolve(), work / "1228.dxbc", "--hlsl-out", work / "shader.hlsl"], check=True)
        subprocess.run([find_dxc(None), "-T", "cs_6_0", "-E", "main", "-O3", "-Fo", work / "1228.dxil", work / "shader.hlsl"], check=True)
        kit = Toolchain()
        obj = kit.compile(ROOT / "tools/reference/page-list/mappings.cpp", work, "/W4", "/WX", f"/I{ROOT / 'runtime/d3d12'}")
        kit.link(work / "probe.exe", obj, "d3d12.lib", "dxgi.lib", "kernel32.lib")
        cases = [("split", True, True, 0), ("shared", False, True, 0),
                 ("fresh", True, False, 0), ("unmapped", True, True, 0xffffffff)]
        expected = {name: workload(work, name, separate, history, previous)
                    for name, separate, history, previous in cases}
        (work / "vsm.inputs").write_text("".join(f"{n} 1228 2 2 4 4 4 4 4 8\n" for n in expected))
        run = host.run(work / "probe.exe", [], timeout=90, capture=True)
        if run.returncode:
            raise RuntimeError((work / "vsm.txt").read_text() if (work / "vsm.txt").exists() else run.stdout)
        results = {}
        for name, buffers in expected.items():
            for i, reference in enumerate(buffers):
                raw = (work / f"{name}.out{i}.bin").read_bytes()
                if raw != reference.tobytes():
                    got = array("I")
                    got.frombytes(raw)
                    bad = [(j, hex(x), hex(y)) for j, (x, y) in enumerate(zip(got, reference)) if x != y]
                    raise ValueError(f"{name} u{i} mismatched {len(bad)} words: {bad[:8]}")
            results[name] = dict(counters=list(buffers[0][:5]), checked_words=sum(map(len, buffers)))
        out = dict(entry=ENTRY, shader="CreateCachedPageMappings", source_sha256=digest(work / "1228.dxbc"),
                   dxil_sha256=digest(work / "1228.dxil"), guest_sha256=digest(work / "probe.exe"),
                   host=host.version, readback=(work / "vsm.txt").read_text().splitlines(), cases=results)
        a.output.parent.mkdir(parents=True, exist_ok=True)
        a.output.write_text(json.dumps(out, indent=2) + "\n")


if __name__ == "__main__":
    main()
