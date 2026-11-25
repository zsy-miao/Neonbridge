#!/usr/bin/env python3
"""Runtime differential tests of dependent packed integer instruction chains.

LLVM assembles synthetic programs, the full translator emits HLSL, and
only input literals and final output stores are instrumented. Each dispatch
tests sixteen independent inputs against the existing CPU reference model.
"""
import argparse
import hashlib
import json
import random
import re
import struct
import subprocess
import sys
from pathlib import Path
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from tools.check_packed_integer_cpu import assembly, assemble_many, generate, EDGE, REGS, ALL_FORMS, VARIANTS, SINGLE, SAT
from tools.reference.rdna2_packed_integer import execute
from tools.frosty_gpu_guest import run_gpu_guest
from tools.check_packed_integer_gpu import load_input

# Six inputs + six outputs per group, with unused guard words around the view.
GROUPS, STRIDE, WORDS, START = 16, 12, 384, 96
INIT = [0x12345670 + i for i in range(len(REGS))]
SAT_EDGE = (0x00000000,0x000100ff,0x00ff0100,0x01000101,0xffff8000,0x7fff8000,0x80007fff,0x0100ffff,0x00ff0001,0x0101fffe,0x800000ff,0xffff00ff)


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def instrument(text):
    marker = "[numthreads(1, 1, 1)]"
    if text.count(marker) != 1 or not text.rstrip().endswith("}"):
        raise ValueError("unexpected compute entry")
    text = text.replace(marker, "RWStructuredBuffer<uint> output : register(u0);\n" + marker)
    for i, reg in enumerate(REGS):
        text = load_input(text, reg, INIT[i], f"output[{STRIDE}u * gid.x + {i}u]")
    stores = "".join(f"    output[{STRIDE}u * gid.x + {len(REGS)+i}u] = v{reg};\n" for i,reg in enumerate(REGS))
    return text.rstrip()[:-1] + stores + "}\n"


def seed_and_expected(ops, rng):
    seed = [rng.getrandbits(32) for _ in range(WORDS)]
    expected = seed.copy()
    edges = SAT_EDGE if any(op[0] in SAT for op in ops) else EDGE
    for group in range(GROUPS):
        values = [edges[(group*3+i*5)%len(edges)] if group < 8 else rng.getrandbits(32)
                  for i in range(len(REGS))]
        offset = START + group*STRIDE
        seed[offset:offset+len(REGS)] = values
        expected[offset:offset+len(REGS)] = values
        expected[offset+len(REGS):offset+STRIDE] = execute(values, ops)
    return struct.pack("<384I", *seed), struct.pack("<384I", *expected)


def expected_from_seed(seed, ops):
    if len(seed) != WORDS*4:
        raise ValueError("invalid seed length")
    values = list(struct.unpack("<384I", seed))
    expected = values.copy()
    for group in range(GROUPS):
        offset = START + group*STRIDE
        expected[offset+len(REGS):offset+STRIDE] = execute(values[offset:offset+len(REGS)], ops)
    return struct.pack("<384I", *expected)


def replay_prefixes(report):
    if report.get("schema") != 1 or not isinstance(report.get("failures"), list):
        raise ValueError("expected a synthetic chain report")
    cases = []
    for i, failure in enumerate(report["failures"]):
        ops = failure["operations"]
        seed = bytes.fromhex(failure["seed_hex"])
        if not 1 <= len(ops) <= 12 or len(seed) != WORDS*4:
            raise ValueError("invalid synthetic chain length or seed")
        for name, dest, sources, lo, hi, clamped in ops:
            supported = (name,clamped) in VARIANTS or name in SAT and not (lo or hi or clamped)
            if not supported or dest not in REGS or len(sources) != ALL_FORMS[name] or any(reg not in REGS for reg in sources) or not 0 <= lo < 2**ALL_FORMS[name] or not 0 <= hi < (2 if name in SINGLE else 2**ALL_FORMS[name]):
                raise ValueError("invalid synthetic operation")
        for n in range(1,len(ops)+1):
            cases.append((f"repro{i:03}_p{n:02}",ops[:n],seed))
    if not 1 <= len(cases) <= 512:
        raise ValueError("prefix replay must contain 1..512 cases")
    return cases


def include_saturate(cases):
    # Preserve the original seeded campaign. In this opt-in campaign every
    # chain includes one newly added pack instruction consuming the previous
    # result, with the original destination so later dependencies stay valid.
    result=[]
    for i,(name,ops,seed) in enumerate(cases):
        ops=list(ops)
        at=1+i%(len(ops)-1)
        ops[at]=(SAT[i%2],ops[at][1],(ops[at-1][1],),0,0,False)
        result.append((name,ops,seed))
    return result


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__)
    for name in ("emitter","llvm-mc","dxc","dxv","provider","proton","prefix","compiler-dir","work","output"):
        ap.add_argument("--"+name, type=Path, required=True)
    ap.add_argument("--adapter", required=True)
    ap.add_argument("--seed", type=int, default=0x43504841)
    ap.add_argument("--chains", type=int, default=128)
    ap.add_argument("--include-saturate", action="store_true", help="insert the new VOP1/VOP3 saturating pack into each chain")
    ap.add_argument("--optimization", choices=("O0","O3"), default="O3")
    ap.add_argument("--replay-prefixes", type=Path, help="isolate the earliest failing prefix from a prior synthetic report")
    a = ap.parse_args(argv)
    if not 1 <= a.chains <= 256 or a.output.exists():
        ap.error("chains 1..256 and fresh output required")
    a.work.mkdir(parents=True, exist_ok=False)
    if a.replay_prefixes and a.include_saturate:
        ap.error("prefix replay preserves the original instructions")
    if a.replay_prefixes:
        cases = replay_prefixes(json.loads(a.replay_prefixes.read_text()))
    else:
        cases = [(f"chain{i:03}",ops,None) for i,ops in enumerate(
            ops for category,_,ops in generate(a.seed,1,a.chains) if category=="chain")]
    if a.include_saturate:
        cases=include_saturate(cases)
    chains = [ops for _,ops,_ in cases]
    programs = [assembly(INIT,ops) for ops in chains]
    encoded = assemble_many((line for lines in programs for line in lines), a.llvm_mc)
    rng = random.Random(a.seed ^ 0x43484149)
    fingerprints = {name:digest(getattr(a,name)) for name in ("emitter","llvm_mc","dxc","dxv","provider")}
    with TemporaryDirectory(dir=a.work) as tmp:
        trial = Path(tmp)
        expected = {}
        rows = []
        for (name,ops,replay_seed),lines in zip(cases,programs):
            words = [f"{word:08x}" for line in lines for word in encoded[line]]
            emitted = subprocess.run([a.emitter,*words],check=True,capture_output=True,text=True,timeout=15).stdout
            source = trial/(name+".hlsl")
            source.write_text(instrument(emitted))
            blob = source.with_suffix(".cso")
            for cmd in ([a.dxc,"-T","cs_6_0","-E","main","-"+a.optimization,"-Fo",blob,source],[a.dxv,blob]):
                subprocess.run(cmd,check=True,capture_output=True,timeout=30)
            if replay_seed is None:
                seed, expected[name] = seed_and_expected(ops,rng)
            else:
                seed = replay_seed
                expected[name] = expected_from_seed(seed,ops)
            (trial/(name+".seed")).write_bytes(seed)
            rows.append(dict(case=name, instructions=len(ops), dxil_sha256=digest(blob)))
        (trial/"cases.txt").write_text("".join(f"{name} 4 structured {GROUPS}\n" for name in expected))
        exe,status,fields = run_gpu_guest(ROOT/"tools/reference/independent-shaders/compute.cpp",
                "packed-chain.exe",trial,a.work,a.proton,a.prefix,a.provider,a.compiler_dir,
                extra_env={"TUXBOX_NULL_PIPELINES":"0"})
        failures = []
        for i,row in enumerate(rows):
            name = row["case"]
            path = trial/(name+".bin")
            actual = path.read_bytes() if path.is_file() else b""
            if actual != expected[name]:
                # Preserve only synthetic reproducers. No title input exists.
                diffs = [j for j in range(min(len(actual),len(expected[name]))//4)
                         if actual[4*j:4*j+4] != expected[name][4*j:4*j+4]]
                failures.append(dict(case=name,operations=chains[i],assembly=programs[i],
                    seed_hex=(trial/(name+".seed")).read_bytes().hex(),
                    expected_hex=expected[name].hex(),actual_hex=actual.hex(),different_words=diffs))
            else:
                row["readback_sha256"] = digest(path)
        if fingerprints != {name:digest(getattr(a,name)) for name in fingerprints}:
            raise ValueError("tool changed during campaign")
        report = dict(schema=1,scope="Synthetic dependent packed integer chains, CPU oracle vs dynamic GPU readback",
             passed=status==0 and fields.get("adapter")==a.adapter and not failures,
             mode="prefix_replay" if a.replay_prefixes else "generated",
             optimization=a.optimization, includes_saturate=a.include_saturate,
             adapter=fields.get("adapter"),guest_exit=status,seed=a.seed,dispatches=len(chains),
             programs=len(chains)*GROUPS,compared_words=len(chains)*WORDS,
             result_words=len(chains)*GROUPS*len(REGS),failures=failures,tools_sha256=fingerprints,
             proton=(a.proton/"version").read_text().strip(),guest_sha256=digest(exe),rows=rows)
        if a.replay_prefixes:
            first = {}
            for failure in failures:
                # Only complete readbacks are semantic mismatch evidence.
                if len(failure["actual_hex"]) == WORDS*8:
                    first.setdefault(failure["case"].split("_p")[0],failure["case"])
            report["first_observed_failing_prefixes"] = first
        with a.output.open("x") as f:
            json.dump(report,f,indent=2); f.write("\n")
    print(f"{report['programs']} programs, {report['compared_words']} words, {len(failures)} mismatched dispatches")
    return int(not report["passed"])


if __name__ == "__main__":
    raise SystemExit(main())
