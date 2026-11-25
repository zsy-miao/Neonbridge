#!/usr/bin/env python3
"""CPU-only differential tests of ordinary packed integer translation.

LLVM assembles native synthetic inputs; the real full shader translator
emits HLSL. Instrument only final register stores; DXC constant-folds the
programs and DXV validates the output. This is not GPU execution or title
coverage. Refusals, invalid outputs and mismatches fail by default.
"""
import argparse
from collections import Counter, defaultdict
from concurrent.futures import ThreadPoolExecutor
import hashlib
import json
from pathlib import Path
import random
import re
import struct
import subprocess
import sys
import time

sys.path.insert(0,str(Path(__file__).resolve().parent))
from reference.rdna2_packed_integer import FORMS, CLAMP, SINGLE, REGS, SAT, execute

EDGE=(0,1,0xffff,0x10000,0x7fff8000,0x80007fff,0xffffffff,0xffff0001,
      0x000f0010,0x001f0020,0x80008000,0x7fff7fff)
VARIANTS=sorted([(name,False) for name in FORMS]+[(name,True) for name in CLAMP])
ALL_FORMS={**FORMS, **dict.fromkeys(SINGLE,2), **dict.fromkeys(SAT,1)}
VARIANTS+=sorted((name,False) for name in SINGLE)
END="s_endpgm"
ENTRY="[numthreads(1, 1, 1)]\nvoid main(uint3 gid : SV_GroupID, uint3 gtid : SV_GroupThreadID) {\n"
STORE=re.compile(r"call void @dx\.op\.bufferStore\.i32\(i32 69, %dx\.types\.Handle %\d+, i32 (\d+), i32 0, i32 (-?\d+), i32 undef, i32 undef, i32 undef, i8 1\)")

def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

def selects(mask,n):
    return "["+",".join(str((mask>>j)&1) for j in range(n))+"]"

def assembly(initial,ops):
    lines=[f"v_mov_b32_e32 v{r}, 0x{x:08x}" for r,x in zip(REGS,initial)]
    for name,d,sources,lo,hi,clamped in ops:
        n=ALL_FORMS[name]
        if name in SAT:
            if n!=1 or lo or hi or clamped:raise ValueError("invalid saturating pack modifiers")
            lines.append(f"{name} v{d}, v{sources[0]}")
            continue
        modifiers=(f" op_sel:{selects(lo | (hi<<2),3)}" if name in SINGLE else
                   f" op_sel:{selects(lo,n)} op_sel_hi:{selects(hi,n)}")
        lines.append(f"{name} v{d}, "+", ".join(f"v{s}" for s in sources)
                     +modifiers+(" clamp" if clamped else ""))
    return lines+[END]

def generate(seed,samples,chains):
    rng=random.Random(seed)
    for name,clamped in VARIANTS:
        n=ALL_FORMS[name]
        for lo in range(2**n):
            for hi in range(2 if name in SINGLE else 2**n):
                for sample in range(samples):
                    if sample==0:
                        values=[EDGE[(lo+hi+j*3)%len(EDGE)] for j in range(6)]
                    elif sample==1:
                        values=[EDGE[(lo*3+hi+j)%len(EDGE)] for j in range(6)]
                    else:
                        values=[rng.getrandbits(32) for _ in REGS]
                    sources=tuple(REGS[:n])
                    dest=sources[(lo+hi+sample)%n] if sample%4!=3 else 9
                    yield "selector",values,[(name,dest,sources,lo,hi,clamped)]
    for i in range(chains):
        values=[rng.choice(EDGE) if (i+j)%3==0 else rng.getrandbits(32) for j in range(6)]
        ops=[]
        last=rng.choice(REGS)
        for j in range(2+i%11):
            name,clamped=rng.choice(VARIANTS)
            n=ALL_FORMS[name]
            sources=[rng.choice(REGS) for _ in range(n)]
            if j:
                sources[j%n]=last
            dest=sources[j%n] if j%3 else rng.choice(REGS)
            ops.append((name,dest,tuple(sources),rng.randrange(2**n),rng.randrange(2 if name in SINGLE else 2**n),clamped))
            last=dest
        yield "chain",values,ops

def assemble_many(lines,llvm):
    unique=list(dict.fromkeys(lines))
    result={}
    for start in range(0,len(unique),4096):
        batch=unique[start:start+4096]
        p=subprocess.run([llvm,"-triple=amdgcn-amd-amdhsa","-mcpu=gfx1030","-show-encoding"],
                         input="\n".join(batch)+"\n",text=True,capture_output=True,timeout=30,check=True)
        enc=re.findall(r"encoding: \[([^\]]+)\]",p.stdout)
        if len(enc)!=len(batch):
            raise ValueError("LLVM encoding count mismatch")
        for ins,data in zip(batch,enc):
            b=bytes(int(x,16) for x in data.split(","))
            if len(b) not in (4,8,12):
                raise ValueError("unexpected instruction length")
            result[ins]=struct.unpack("<"+"I"*(len(b)//4),b)
    return result

def parse_stores(text,expected_count):
    out={}
    for line in text.splitlines():
        if "call void @dx.op.bufferStore" not in line:
            continue
        m=STORE.search(line)
        if not m:
            raise ValueError("unexpected or nonconstant output store")
        index,value=map(int,m.groups())
        if index in out:
            raise ValueError("duplicate output store")
        out[index]=value&0xffffffff
    if set(out)!=set(range(expected_count)):
        raise ValueError("missing or extra output store")
    return [out[i] for i in range(expected_count)]

def split_hlsl(text):
    if text.count(ENTRY)!=1 or not text.rstrip().endswith("}"):
        raise ValueError("unexpected compute entry")
    header,body=text.split(ENTRY)
    return header,body.rstrip()[:-1]

def compare(args):
    start=time.monotonic()
    cases=[]
    for category,initial,ops in generate(args.seed,args.samples,args.chains):
        cases.append({"category":category,"assembly":assembly(initial,ops),"expected":execute(initial,ops)})
    encoded=assemble_many((line for case in cases for line in case["assembly"]),args.llvm_mc)
    for i,case in enumerate(cases):
        case["id"]=i
        case["words"]=[f"{word:08x}" for line in case["assembly"] for word in encoded[line]]
    case_hash=hashlib.sha256(json.dumps(cases,sort_keys=True,separators=(",",":")).encode()).hexdigest()
    tools={n:digest(getattr(args,n)) for n in ("emitter","llvm_mc","dxc","dxv")}
    groups=defaultdict(list)
    failures=[]
    def emit(case):
        p=subprocess.run([args.emitter,*case["words"]],text=True,capture_output=True,timeout=10)
        if p.returncode==65:
            return None,None,{"id":case["id"],"status":"refused","reason":p.stderr.strip()}
        if p.returncode:
            return None,None,{"id":case["id"],"status":"tool_error","reason":p.stderr[-300:]}
        header,body=split_hlsl(p.stdout)
        return header,body,None
    with ThreadPoolExecutor(max_workers=args.jobs) as pool:
        for case,(header,body,error) in zip(cases,pool.map(emit,cases)):
            if error:
                failures.append(error)
            else:
                groups[header].append((case,body))
    batches=[]
    for header,items in groups.items():
        for i in range(0,len(items),48):
            batches.append((header,items[i:i+48]))
    def verify(item):
        index,(header,items)=item
        path=args.work/f"batch-{index:05d}.hlsl"
        pieces=[header,"RWStructuredBuffer<uint> output : register(u0);\n",ENTRY]
        for j,(case,body) in enumerate(items):
            pieces.extend(["{\n",body])
            pieces.extend(f"output[{j*6+k}u] = v{reg};\n" for k,reg in enumerate(REGS))
            pieces.append("}\n")
        pieces.append("}\n")
        path.write_text("".join(pieces))
        blob,asm=path.with_suffix(".cso"),path.with_suffix(".ll")
        for cmd in ([args.dxc,"-T","cs_6_0","-E","main","-O3","-Fo",blob,"-Fc",asm,path],[args.dxv,blob]):
            p=subprocess.run(cmd,text=True,capture_output=True,timeout=30)
            if p.returncode:
                return 0,[{"id":case["id"],"status":"tool_error","reason":(p.stderr or p.stdout)[-500:]} for case,_ in items],None
        try:
            values=parse_stores(asm.read_text(),len(items)*6)
        except ValueError as e:
            return 0,[{"id":case["id"],"status":"unobservable","reason":str(e)} for case,_ in items],None
        errors=[]
        for j,(case,_) in enumerate(items):
            observed=values[j*6:(j+1)*6]
            if observed!=case["expected"]:
                errors.append({"id":case["id"],"status":"mismatch","actual":observed})
        return len(values),errors,{"cases":len(items),"dxil_sha256":digest(blob)}
    checked=0
    receipts=[]
    with ThreadPoolExecutor(max_workers=args.jobs) as pool:
        for count,errors,receipt in pool.map(verify,enumerate(batches)):
            checked+=count
            failures.extend(errors)
            if receipt:
                receipts.append(receipt)
    if tools!={n:digest(getattr(args,n)) for n in tools}:
        raise ValueError("tool changed during test")
    for error in failures:
        error.update(cases[error["id"]])
    report={"schema":1,"scope":"CPU only: independent packed integer model vs full translator, DXC constant folding and DXV",
            "seed":args.seed,"samples":args.samples,"chains":args.chains,"variants":len(VARIANTS),
            "categories":dict(Counter(c["category"] for c in cases)),
            "programs":len(cases),"passed":len(cases)-len(failures),"checked_words":checked,
            "distinct_llvm_instructions":len(encoded),"corpus_sha256":case_hash,
            "tool_sha256":tools,"failure_counts":dict(Counter(e["status"] for e in failures)),
            "failures":failures,"batches":receipts,"seconds":time.monotonic()-start}
    return report

def main():
    ap=argparse.ArgumentParser(description=__doc__)
    for name in ("emitter","llvm-mc","dxc","dxv","work","output"):
        ap.add_argument("--"+name,type=Path,required=True)
    ap.add_argument("--seed",type=int,default=0x5041434b)
    ap.add_argument("--samples",type=int,default=4)
    ap.add_argument("--chains",type=int,default=2048)
    ap.add_argument("--jobs",type=int,choices=(1,2),default=2)
    args=ap.parse_args()
    if not 1<=args.samples<=8 or not 0<=args.chains<=4096 or args.output.exists():
        ap.error("samples 1..8, chains 0..4096, fresh output")
    args.work.mkdir(parents=True,exist_ok=False)
    report=compare(args)
    with args.output.open("x") as f:
        json.dump(report,f,indent=2);f.write("\n")
    print(f"{report['passed']}/{report['programs']} programs; {report['checked_words']} words; failures {report['failure_counts']}; {report['seconds']:.2f}s")
    return int(bool(report["failures"]))
if __name__=="__main__":
    raise SystemExit(main())
