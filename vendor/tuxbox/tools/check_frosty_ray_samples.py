#!/usr/bin/env python3
"""Check image operands and shader-embedded tables from packaged ray materials on a GPU."""
import argparse
import json
import re
import struct
import subprocess
import sys
from pathlib import Path
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from build import Toolchain, find_dxc
from check_frosty_blas import extract_material_states
from project_tuxbox.host import Host
from project_tuxbox.inspect import digest

# The inputs and destinations below come from the original RDNA2 operands.
# Compile the actual emitted instruction including its destination swizzle.
CASES = (
    (374, 4, 0, "i == 0", "v[70]=asuint(.3125); v[71]=asuint(.8125); v[72]=asuint(.625);", 66, (66, 67, 68, 69)),
    (2129, 6, 2, "i >= 1 && i <= 6", "v[45]=asuint(1.6875); v[12]=asuint(1.3125); v[7]=asuint(float(i-1));", 53, (53, 54, 55)),
    (3148, 5, 0, "i == 7", "v[42]=asuint(.5625); v[45]=asuint(.1875); v[58]=asuint(2.);", 55, (55, 56, 57)),
    (4364, 11, 5, "i >= 8 && i <= 10", "v[4]=asuint(.6875); v[9]=asuint(.3125); v[39]=asuint(i==8 ? 0. : i==9 ? .14 : .27);", 26, (26,)),
    (1671, 7, None, "i == 11", "v[78]=3; v[79]=5;", 4, (4,)),
)


def literal_block(work, translator):
    original = work / "material-363.sptr"
    hlsl = work / "363.hlsl"
    subprocess.run([translator, original, "--hlsl-out", hlsl], check=True, capture_output=True)
    text = hlsl.read_text()
    helpers = [re.search(r"^uint tbMaterialCode\([^\n]+", text, re.M).group(),
               re.search(r"^uint2 tb_ray_shift_left\(.*?^}", text, re.M | re.S).group()]
    # This guest block gets three PCs, adds table relocations, looks up an index
    # and reads two coordinate components. The final load overwrites its address.
    block = re.search(r"case 43704u: \{ nextBlock = 43892u;\n(.*?)\nbreak;\n}", text, re.S).group(1)
    branch = f"""if (i >= 12) {{
        uint s[64]=(uint[64])0, exec=1, vcc=0, scc=0;
        v[3] = i - 12; v[2] = i - 12;
        {block}
        result[i] = uint4(v[11], v[12], v[3], v[7]);
    }}"""
    return helpers, branch


def prepare(work, translator, dxc, mount):
    extract_material_states(mount, work, [363, *(case[0] for case in CASES)])
    declarations, branches, inputs = set(), [], {}
    cube = ""
    for entry, slot, sampler, condition, init, first, output in CASES:
        source = work / f"material-{entry}.sptr"
        hlsl = work / f"{entry}.hlsl"
        subprocess.run([translator, source, "--hlsl-out", hlsl], check=True, capture_output=True)
        text = hlsl.read_text()
        inputs[entry] = digest(source)
        srv = f"tbSrv{slot}"
        declarations.add(re.search(rf"^Texture\w+<\w+> {srv} : register\(.+?;", text, re.M).group())
        if sampler is not None:
            declarations.add(re.search(rf"^SamplerState tbSampler{sampler} : register\(.+?;", text, re.M).group())
        if entry == 2129:
            cube = re.search(r"^float3 tb_cube_direction\(.*?^}", text, re.M | re.S).group()
        blocks = [line for line in text.splitlines() if line.startswith("{ uint4 sampleBits =")
                  and f"{srv}." in line and f"; v[{first}] = sampleBits[" in line]
        if not blocks:
            raise ValueError(f"Missing decoded sampling operation for {entry}")
        out = [f"v[{reg}]" for reg in output] + ["0xdeadbeefu"] * (4 - len(output))
        branches.append(f"if ({condition}) {{ {init}\n{blocks[0]}\nresult[i] = uint4({','.join(out)}); }}")
    helpers, literal = literal_block(work, translator)
    inputs[363] = digest(work / "material-363.sptr")
    code = "\n".join([*sorted(declarations), cube, *helpers, "RWStructuredBuffer<uint4> result : register(u0);",
                      "[numthreads(1,1,1)] void main(uint i:SV_DispatchThreadID) { uint v[96]=(uint[96])0;",
                      *branches, literal, "}"])
    wrapper = work / "ray-samples.hlsl"
    wrapper.write_text(code)
    subprocess.run([dxc, "-T", "cs_6_5", "-E", "main", "-Fo", work / "ray-samples.cso", wrapper], check=True)
    return inputs


def expected(original):
    # Point sampling chooses floor(uv * extent); the cube inputs select column
    # five, row two on each face. The gradients choose mip 0, 1 and 2 of t11.
    cases = [(2, 6, 2, 0, 4), *((5, 2, face, 0, 3) for face in range(6)),
             (4, 1, 2, 0, 3), (5, 2, 0, 0, 1), (2, 1, 0, 1, 1), (1, 0, 0, 2, 1)]
    result = []
    for x, y, z, mip, channels in cases:
        first = 1 + x + 16*y + 128*z + 1024*mip
        result.append(tuple(struct.unpack("<I", struct.pack("<f", first + 4096*c))[0] for c in range(channels)) +
                      (0xdeadbeef,) * (4-channels))
    result.append((1 + 3 + 16*5 + 4096*3, 0xdeadbeef, 0xdeadbeef, 0xdeadbeef))
    # The SPTR has its main ISA at byte 0x2e74. The 32 original literal words
    # start at main + 55040 + 0x2a8. Read the independently located guest data.
    words = struct.unpack_from("<32I", original.read_bytes(), 67612)
    for invocation in range(36):
        index = words[8 + invocation % 18]
        result.append((index, words[index], words[4 + index], 0))
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("mount", "xshader", "proton", "prefix", "output"):
        parser.add_argument(f"--{name}", type=Path, required=True)
    args = parser.parse_args()
    (ROOT / "build").mkdir(exist_ok=True)
    with TemporaryDirectory(dir=ROOT / "build") as temp:
        work = Path(temp)
        shader_hashes = prepare(work, args.xshader.resolve(), find_dxc(None), args.mount)
        tc = Toolchain()
        obj = tc.compile(ROOT / "tools/reference/independent-shaders/ray-samples.cpp", work, "/W4", "/WX",
                         f"/I{ROOT / 'runtime/d3d12'}")
        exe = work / "ray-samples.exe"
        tc.link(exe, obj, "d3d12.lib", "dxgi.lib", "kernel32.lib")
        host = Host(None, args.proton, args.prefix)
        run = host.run(exe, [], capture=True, timeout=90)
        if run.returncode:
            raise RuntimeError((work / "result.txt").read_text() if (work / "result.txt").exists() else run.stdout)
        readback = work / "ray-samples.bin"
        actual = list(struct.iter_unpack("<4I", readback.read_bytes()))
        wanted = expected(work / "material-363.sptr")
        if actual != wanted:
            for case, (got, want) in enumerate(zip(actual, wanted, strict=True)):
                if got != want:
                    raise ValueError(f"Material texture case {case} returned {got}, expected {want}")
        adapter = (work / "result.txt").read_text().strip().split("=")[1]
        result = {"adapter": adapter, "host": host.version, "states": shader_hashes,
                  "sample_shader_sha256": digest(work / "ray-samples.cso"), "readback_sha256": digest(readback),
                  "cases": len(actual), "mismatches": 0}
        args.output.write_text(json.dumps(result, indent=2) + "\n")


if __name__ == "__main__":
    main()
