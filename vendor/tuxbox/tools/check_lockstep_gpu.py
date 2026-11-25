#!/usr/bin/env python3
"""Check on a real GPU that translated branches are decisions of the whole wave.

Each synthetic wave32 program branches on a lane mask, then runs a reduction
behind a mask branch, which the translator can only express by branching with
the whole wave. One program also runs as a ray tracing resolve shader, whose
interleaved loops go through the block dispatcher. The reference models the console: one program counter per wave,
shared scalar registers and per-lane vector writes under EXEC. Only the
translator's input and output are instrumented. No game data is needed.
"""
import argparse
import json
from pathlib import Path
import struct
import subprocess
import sys
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(ROOT / "tools"))
from check_packed_integer_gpu import assemble  # noqa: E402
from check_wave_guard import cases  # noqa: E402
from frosty_gpu_guest import run_gpu_guest  # noqa: E402
from project_tuxbox.inspect import digest  # noqa: E402

WIDTH = 32
LIMIT = 1000
# The OR-saveexec, DPP and lane-read reduction of the translator's own fixture:
# s7 receives the sum of the active lanes' v0.
REDUCTION = [0xbe803dc1, 0xd5010002, 0x00020080, 0x4a0404e9, 0xde54c102, 0x4a0404e9, 0x93601202,
             0x4a0404e9, 0x00092402, 0x4a0404fa, 0xff094002, 0xd7780003, 0x02010102, 0x4a040503,
             0xd7600007, 0x00010102, 0xbefe0300]
OUTPUTS = ("s20", "s21", "v10", "s7")
COMPARE = f"v_cmp_lt_u32_e32 vcc_lo, 0x{LIMIT:x}, v0"


def label(name):
    return ("label", name)


def branch(mnemonic, target):
    return ("branch", mnemonic, target)


# Every program may use s10-s11, s20-s23, v10-v12. s12 holds the entry EXEC.
BODIES = {
    # A scalar write behind a branch on VCC.
    "vccz": [COMPARE, branch("s_cbranch_vccz", "done"), "s_mov_b32 s20, 1", label("done"),
             "v_mov_b32_e32 v10, s20"],
    "vccnz": [COMPARE, "s_mov_b32 s20, 1", branch("s_cbranch_vccnz", "done"), "s_mov_b32 s20, 2", label("done")],
    # A scalar write in an EXEC-guarded block that no lane may enter.
    "execz": [COMPARE, "s_and_saveexec_b32 s10, vcc_lo", branch("s_cbranch_execz", "done"),
              "s_mov_b32 s20, 7", "v_mov_b32_e32 v10, 5", label("done"), "s_mov_b32 exec_lo, s10"],
    # The condition code of a mask operation, and a whole mask compared with zero.
    "scc": [COMPARE, "s_and_b32 s11, vcc_lo, exec_lo", branch("s_cbranch_scc0", "first"),
            "s_mov_b32 s20, 3", label("first"), "s_cmp_eq_u32 vcc_lo, 0", branch("s_cbranch_scc1", "done"),
            "s_mov_b32 s21, 4", label("done")],
    # Lanes leave a loop at different iterations; the wave stays until the last.
    "loop": ["v_and_b32_e32 v11, 7, v0", "s_mov_b32 s10, exec_lo", label("again"),
             "s_add_u32 s20, s20, 1", "v_cmp_lt_u32_e64 s11, s20, v11", "s_and_b32 exec_lo, exec_lo, s11",
             "v_add_nc_u32_e32 v10, 1, v10", branch("s_cbranch_execnz", "again"), "s_mov_b32 exec_lo, s10"],
    # Interleaved loops, which take the block dispatcher.
    "dispatch": [COMPARE, label("outer"), "s_add_u32 s20, s20, 1", label("inner"),
                 branch("s_cbranch_vccz", "quiet"), "s_add_u32 s21, s21, 1", label("quiet"),
                 "s_and_saveexec_b32 s10, vcc_lo", branch("s_cbranch_execz", "empty"),
                 "v_add_nc_u32_e32 v10, 1, v10", label("empty"), "s_mov_b32 exec_lo, s10",
                 "s_add_u32 s22, s22, 1", "s_cmp_lt_u32 s20, 3", branch("s_cbranch_scc1", "outer"),
                 "s_cmp_lt_u32 s22, 6", branch("s_cbranch_scc1", "inner")],
}


def program(body, resolve=False):
    """Assembler lines and branch positions of one complete program."""
    return [
        "s_mov_b32 s12, exec_lo",
        # The resolve mode is selected by a compare into an odd scalar register.
        *(["v_cmp_eq_u32_e64 s31, 0, v1"] if resolve else []), *body, "s_mov_b32 exec_lo, s12",
        # No active lane skips the reduction. A branch on VCC around it is what
        # the targeted guards refuse.
        "v_cmp_eq_u32_e32 vcc_lo, v1, v1", branch("s_cbranch_vccz", "end"),
        ("words", REDUCTION), label("end"), "s_endpgm",
    ]


def words_of(items, llvm_mc):
    lines = [item for item in items if isinstance(item, str)]
    sizes, encoded = [], []
    for line in lines:
        encoded.append(assemble([line], llvm_mc))
    offsets, position, queue = {}, 0, iter(encoded)
    for item in items:
        if isinstance(item, str):
            sizes.append(next(queue))
            position += len(sizes[-1])
        elif item[0] == "label":
            offsets[item[1]] = position
        else:
            position += 1 if item[0] == "branch" else len(item[1])
    words, queue = [], iter(encoded)
    for item in items:
        if isinstance(item, str):
            words += next(queue)
        elif item[0] == "words":
            words += item[1]
        elif item[0] == "branch":
            jump = (offsets[item[2]] - len(words) - 1) & 0xffff
            words += assemble([f"{item[1]} {jump}"], llvm_mc)
    return words


def model(name, active, values):
    """The console's registers after `BODIES[name]`, as one row per lane."""
    lanes = range(len(values))
    taken = [active[i] and values[i] > LIMIT for i in lanes]
    some = any(taken)
    s20 = s21 = 0
    v10 = [0] * len(values)
    if name == "vccz":
        s20 = int(some)
        v10 = [s20 if active[i] else 0 for i in lanes]
    elif name == "vccnz":
        s20 = 1 if some else 2
    elif name == "execz":
        s20 = 7 if some else 0
        v10 = [5 if taken[i] else 0 for i in lanes]
    elif name == "scc":
        s20, s21 = (3, 4) if some else (0, 0)
    elif name == "loop":
        limits = [values[i] & 7 for i in lanes]
        running = list(active)
        while True:
            s20 += 1
            running = [running[i] and s20 < limits[i] for i in lanes]
            v10 = [v10[i] + int(running[i]) for i in lanes]
            if not any(running):
                break
    elif name == "dispatch":
        s22, at = 0, "outer"
        while at:
            if at == "outer":
                s20 += 1
            s21 += int(some)
            v10 = [v10[i] + int(taken[i]) for i in lanes]
            s22 += 1
            at = "outer" if s20 < 3 else "inner" if s22 < 6 else None
    else:
        raise ValueError(name)
    total = sum(v for v, a in zip(values, active) if a) & 0xffffffff if any(active) else 0
    return [(s20, s21, v10[i], total) for i in lanes]


def instrument(hlsl, resolve=False):
    if resolve:
        # This mode keeps whole masks and numbers its lanes by the host wave.
        start = "    v0 = WaveGetLaneIndex(); v1 = gtid.y; v2 = gtid.z; uint tb_lane_bit = 1u << v0;"
        load = (start + "\n    uint tb_io = v0; uint input = output[tb_io];\n"
                "    exec_lo = WaveActiveBallot((input >> 31) != 0u).x; v0 = input & 0x7fffffffu;")
        index, voted = "tb_io", "exec_lo == 0u"
    else:
        start = "    v0 = gtid.x; v1 = gtid.y; v2 = gtid.z;"
        load = ("    uint input = output[gtid.x];\n    exec_lo = input >> 31;\n"
                "    v0 = input & 0x7fffffffu; v1 = gtid.y; v2 = gtid.z;")
        index, voted = "gtid.x", "WaveActiveAnyTrue"
    if hlsl.count(start) != 1 or "[WaveSize(32)]" not in hlsl or voted not in hlsl:
        raise ValueError("fixture must branch with a whole wave32")
    hlsl = hlsl.replace(start, load)
    stores = []
    for column, register in enumerate(OUTPUTS):
        # A register the program never touches is not declared.
        value = register if register in hlsl.split() or f" {register} " in hlsl or f" {register}=" in hlsl else "0u"
        stores.append(f"    output[{32 * (column + 1)}u + {index}] = {value};\n")
    end = hlsl.rstrip().rfind("}")
    return "RWBuffer<uint> output : register(u0);\n" + hlsl[:end] + "".join(stores) + "}\n"


def inputs():
    yield from cases(WIDTH)
    yield "below", 0xffffffff, list(range(WIDTH))
    yield "one_above", 0xffffffff, [LIMIT + 1 if lane == 17 else lane for lane in range(WIDTH)]
    yield "inactive_above", 0x0000ffff, [lane if lane < 16 else LIMIT + lane for lane in range(WIDTH)]


def seed_and_expected(name, mask, values):
    seed = [0x76540000 + i for i in range(384)]
    active = [bool((mask >> lane) & 1) for lane in range(WIDTH)]
    seed[96:96 + WIDTH] = [v | (int(a) << 31) for v, a in zip(values, active)]
    expected = seed.copy()
    for lane, row in enumerate(model(name, active, values)):
        for column, word in enumerate(row):
            expected[96 + WIDTH * (column + 1) + lane] = word
    return struct.pack("<384I", *seed), struct.pack("<384I", *expected)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("emitter", "llvm-mc", "dxc", "dxv", "provider", "proton", "prefix", "compiler-dir", "work",
                 "output"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--adapter", required=True)
    args = parser.parse_args(argv)
    work = args.work.resolve()
    work.mkdir(parents=True, exist_ok=True)
    with TemporaryDirectory(dir=work) as tmp:
        trial = Path(tmp)
        expected, shaders = {}, {}
        for name, body, resolve in [*((name, name, False) for name in BODIES), ("resolve", "dispatch", True)]:
            words = words_of(program(BODIES[body], resolve), args.llvm_mc)
            emitted = subprocess.check_output(
                [args.emitter.resolve(), *(["resolve"] if resolve else []), str(WIDTH),
                 *(f"{word:08x}" for word in words)], text=True, timeout=30)
            source = trial / f"{name}.hlsl"
            source.write_text(instrument(emitted, resolve))
            blob = trial / f"{name}.cso"
            subprocess.run([args.dxc, "-T", "cs_6_6", "-E", "main", "-Fo", blob, source], check=True, timeout=30,
                           capture_output=True)
            subprocess.run([args.dxv, blob], check=True, timeout=30, capture_output=True)
            shaders[name] = dict(hlsl_sha256=digest(source), dxil_sha256=digest(blob),
                                 dispatched="tb_pc" in emitted)
            for case, mask, values in inputs():
                label_ = f"{name}_{case}"
                seed, expected[label_] = seed_and_expected(body, mask, values)
                (trial / f"{label_}.seed").write_bytes(seed)
                (trial / f"{label_}.cso").hardlink_to(blob)
        if not (shaders["dispatch"]["dispatched"] and shaders["resolve"]["dispatched"]):
            raise ValueError("the interleaved fixture must use the block dispatcher")
        (trial / "cases.txt").write_text("".join(f"{name} 0 uint 1\n" for name in expected))
        exe, status, fields = run_gpu_guest(
            ROOT / "tools/reference/independent-shaders/compute.cpp", "lockstep.exe", trial, work,
            args.proton.resolve(), args.prefix, args.provider, args.compiler_dir)
        wrong = {}
        for name, reference in expected.items():
            path = trial / f"{name}.bin"
            actual = path.read_bytes() if path.is_file() else b""
            if actual != reference:
                pairs = enumerate(zip(struct.unpack(f"<{len(actual) // 4}I", actual), struct.unpack("<384I", reference)))
                wrong[name] = next((dict(word=i, got=a, wanted=b) for i, (a, b) in pairs if a != b), "no readback")
        passed = status == 0 and fields.get("adapter") == args.adapter and not wrong
        report = dict(passed=passed, scope="Synthetic wave32 programs branching with the whole wave",
                      adapter=fields.get("adapter"), guest_exit=status, cases=len(expected),
                      compared_words=len(expected) * 384, wrong=wrong, shaders=shaders,
                      emitter_sha256=digest(args.emitter), guest_sha256=digest(exe))
        if fields.get("error"):
            report["error"] = fields["error"]
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(report, indent=2) + "\n")
    for name, detail in wrong.items():
        print(f"WRONG {name}: {detail}")
    if not passed:
        print(f"FAIL: {len(wrong)} of {len(expected)} cases, guest exit {status}")
        return 1
    print(f"PASS: {len(expected)} cases, {len(expected) * 384} words, adapter {fields['adapter']}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
