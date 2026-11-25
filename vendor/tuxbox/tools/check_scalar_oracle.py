#!/usr/bin/env python3
"""Compare independent scalar simulation with observable production HLSL stores.

The bridge executable is the compiled Rust library test binary, built with
cargo test --no-run --lib. This runs DXC/DXV on CPU, not a GPU interpreter.
All generated programs and mismatches are reproducible by case ID and seed.
"""
import argparse
from concurrent.futures import ThreadPoolExecutor
from collections import Counter
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys

sys.path.insert(0, str(Path(__file__).resolve().parent))
from reference.rdna2_scalar import execute, MASK


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def load(directory):
    manifest = json.loads((directory/"manifest.json").read_text())
    program = directory/"programs.txt"
    if manifest["schema"] != 1 or manifest["program_sha256"] != digest(program):
        raise ValueError("corpus fingerprint mismatch")
    rows = manifest["rows"]
    lines = program.read_text().splitlines()
    if not rows or len(rows) != len(lines):
        raise ValueError("corpus size mismatch")
    for n,(row,line) in enumerate(zip(rows,lines)):
        ident,*words = line.split()
        if ident != f"{n:06d}" or row["id"] != ident:
            raise ValueError("case identity mismatch")
        expected = execute([int(word,16) for word in words])
        if expected != row["expected"]:
            raise ValueError("manifest does not match independent model")
    if dict(Counter(row["category"] for row in rows)) != manifest["counts"]:
        raise ValueError("category counts do not match corpus")
    return manifest


def stores(disassembly):
    pattern = re.compile(
        r"call void @dx\.op\.bufferStore\.i32\(i32 69, %dx\.types\.Handle %\d+, "
        r"i32 (\d+), i32 0, i32 (-?\d+), i32 undef, i32 undef, i32 undef, i8 1\)")
    result = {}
    for line in disassembly.splitlines():
        if "call void @dx.op.bufferStore" not in line:
            continue
        match = pattern.search(line)
        if not match:
            raise ValueError("nonconstant/unexpected observable store: " + line[:200])
        index,value = map(int,match.groups())
        if index in result:
            raise ValueError("duplicate output store")
        result[index] = value & MASK
    return result


def check(corpus, bridge, dxc, dxv, work, jobs):
    manifest = load(corpus)
    rows = manifest["rows"]
    tools = {name:digest(path) for name,path in (("bridge",bridge),("dxc",dxc),("dxv",dxv))}
    work.mkdir(parents=True,exist_ok=False)
    env = dict(os.environ,TUXBOX_ORACLE_PROGRAMS=str((corpus/"programs.txt").resolve()),
               TUXBOX_ORACLE_OUTPUT=str(work.resolve()))
    result = subprocess.run([bridge,"--exact","hlsl::cpu_oracle::scalar_oracle_bridge","--nocapture"],
                            env=env,timeout=90,capture_output=True,text=True)
    if result.returncode:
        raise RuntimeError("native bridge failed: " + (result.stderr or result.stdout)[-3000:])
    status = (work/"status.tsv").read_text().splitlines()
    if len(status) != len(rows):
        raise ValueError("bridge omitted cases")
    batches = {}
    failures = []
    for row,line in zip(rows,status):
        ident,kind,*detail = line.split("\t")
        if ident != row["id"]:
            raise ValueError("bridge reordered cases")
        if kind == "refused" and len(detail)==1:
            failures.append({"id":ident,"category":row["category"],"status":"refused","reason":detail[0]})
        elif kind == "compiled" and len(detail)==2:
            batch,offset = map(int,detail)
            group = batches.setdefault(batch,[])
            if offset != len(group) or not 0 <= offset < 64:
                raise ValueError("invalid bridge slot")
            group.append(row)
        else:
            raise ValueError("invalid bridge response")
    if set(batches) != set(range(len(batches))):
        raise ValueError("missing batch")
    def verify(item):
        batch,group = item
        source = work/f"batch-{batch:05d}.hlsl"
        blob,asm = source.with_suffix(".cso"),source.with_suffix(".ll")
        for cmd in ([dxc,"-T","cs_6_0","-E","main","-O3","-Fo",blob,"-Fc",asm,source],
                    [dxv,blob]):
            proc = subprocess.run(cmd,capture_output=True,text=True,timeout=30)
            if proc.returncode:
                return 0,[{"id":r["id"],"category":r["category"],"status":"tool_error",
                           "reason":(proc.stderr or proc.stdout)[-1000:]} for r in group],None
        try:
            actual = stores(asm.read_text())
        except ValueError as exc:
            return 0,[{"id":r["id"],"category":r["category"],"status":"unobservable",
                       "reason":str(exc)} for r in group],None
        if set(actual) != set(range(len(group)*9)):
            return 0,[{"id":r["id"],"category":r["category"],"status":"unobservable",
                       "reason":"missing or extra output stores"} for r in group],None
        errors=[]
        for n,row in enumerate(group):
            got = [actual[n*9+k] for k in range(9)]
            if got != row["expected"]:
                errors.append({"id":row["id"],"category":row["category"],"status":"mismatch",
                               "expected":row["expected"],"actual":got})
        return len(group)*9,errors,{"batch":batch,"dxil_sha256":digest(blob),"cases":len(group)}
    count=0
    receipts=[]
    with ThreadPoolExecutor(max_workers=jobs) as pool:
        for checked,errors,receipt in pool.map(verify,sorted(batches.items())):
            count += checked
            failures.extend(errors)
            if receipt:
                receipts.append(receipt)
    if tools != {name:digest(path) for name,path in (("bridge",bridge),("dxc",dxc),("dxv",dxv))}:
        raise ValueError("compiler tools changed")
    return {"scope":"CPU scalar ISA model vs production emitter, DXC constant folding and DXV; no GPU or retail coverage",
            "program_sha256":manifest["program_sha256"],"seed":manifest["seed"],"profile":manifest["profile"],
            "categories":manifest["counts"],"tool_sha256":tools,"cases":len(rows),
            "passed":len(rows)-len(failures),"checked_words":count,
            "failures":sorted(failures,key=lambda r:r["id"]),"batches":receipts}


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    for name in ("corpus","bridge","dxc","dxv","work","output"):
        parser.add_argument("--"+name,type=Path,required=True)
    parser.add_argument("--jobs",type=int,default=2)
    args=parser.parse_args()
    if not 1<=args.jobs<=8 or args.output.exists():
        parser.error("jobs 1..8 and fresh output required")
    if args.output.resolve().is_relative_to(args.corpus.resolve()) or args.work.resolve().is_relative_to(args.corpus.resolve()):
        parser.error("work and report must be outside the input corpus")
    report=check(*(getattr(args,n).resolve() for n in ("corpus","bridge","dxc","dxv","work")),args.jobs)
    with args.output.open("x") as stream:
        json.dump(report,stream,indent=2)
        stream.write("\n")
    print(f"{report['passed']}/{report['cases']} cases passed; {report['checked_words']} words checked; {len(report['failures'])} failures")
    return int(bool(report["failures"]))


if __name__=="__main__":
    raise SystemExit(main())
