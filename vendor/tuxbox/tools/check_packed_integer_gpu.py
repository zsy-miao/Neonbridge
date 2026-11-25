#!/usr/bin/env python3
"""LLVM-assembled VOP3P ordinary integer controls on a D3D12 GPU.

All low/high source selectors for five ordinary forms or seven saturating
arithmetic forms; each result aliases one source. Synthetic dynamic inputs and guards.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import random
import re
import struct
import subprocess
import sys
from tempfile import TemporaryDirectory

ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT))
from frosty_gpu_guest import run_gpu_guest

OPS={"v_pk_add_i16":2,"v_pk_mad_i16":3,"v_pk_mad_u16":3,
     "v_pk_max_u16":2,"v_pk_min_u16":2}
CLAMP_OPS={"v_pk_add_i16":2,"v_pk_sub_i16":2,"v_pk_mad_i16":3,"v_pk_add_u16":2,"v_pk_sub_u16":2,"v_pk_mul_lo_u16":2,"v_pk_mad_u16":3}
INIT=(0x12345678,0x23456789,0x3456789a)
STRIDE=11
GROUPS=16

def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

def assemble(lines,llvm):
    result=subprocess.run([str(llvm),"-triple=amdgcn-amd-amdhsa","-mcpu=gfx1030","-show-encoding"],
                          input="\n".join(lines)+"\n",text=True,capture_output=True,check=True,timeout=20)
    encoded=re.findall(r"encoding: \[([^\]]+)\]",result.stdout)
    if len(encoded)!=len(lines):
        raise ValueError("LLVM encoding count differs")
    return [word for line in encoded
            for word in struct.unpack("<"+"I"*(len(line.split(","))//4),
                                      bytes(int(v,16) for v in line.split(",")))]

def source_select(sel,n):
    return "["+",".join(str((sel>>j)&1) for j in range(n))+"]"

def variant(name,n,lo,hi,reg,alias,clamped=False):
    sources=[f"v{4+j}" for j in range(n)]
    lines=[f"v_mov_b32_e32 v{reg}, {sources[alias]}"]
    sources[alias]=f"v{reg}"
    lines.append(f"{name} v{reg}, "+", ".join(sources)+
                 f" op_sel:{source_select(lo,n)} op_sel_hi:{source_select(hi,n)}"+(" clamp" if clamped else ""))
    return lines

def reference(name,values,lo,hi,clamped=False):
    def half(sel):
        a,b,*c=[(v>>(16*((sel>>j)&1)))&0xffff for j,v in enumerate(values[:(CLAMP_OPS if clamped else OPS)[name]])]
        if clamped:
            signed_op=name.endswith("_i16")
            if signed_op:
                a,b,*c=[x-65536 if x>=32768 else x for x in [a,b,*c]]
            if "_add_" in name:
                result=a+b
            elif "_sub_" in name:
                result=a-b
            elif "_mad_" in name:
                result=a*b+c[0]
            elif "_mul_" in name:
                result=a*b
            else:
                raise ValueError(name)
            lower,upper=(-32768,32767) if signed_op else (0,65535)
            return max(lower,min(upper,result)) & 65535
        if name=="v_pk_add_i16":
            return (a+b)&0xffff
        if "mad" in name:
            if name=="v_pk_mad_i16":
                signed=lambda x:x-65536 if x>=32768 else x
                return (signed(a)*signed(b)+signed(c[0]))%65536
            return (a*b+c[0])%65536
        return max(a,b) if name=="v_pk_max_u16" else min(a,b)
    return half(lo)|(half(hi)<<16)

def translate(emitter,words):
    return subprocess.run([str(emitter),*[f"{v:08x}" for v in words]],
                          text=True,capture_output=True,timeout=20)

def load_input(text, reg, value, source):
    """Make a test input dynamic: the one instruction that gives `v{reg}` its
    literal `value` reads `source` instead. The translator writes predicated
    instructions singly or grouped in a block, so only the assignment is matched."""
    text, found = re.subn(rf"\b(v{reg} = )0x{value:x}u;", rf"\g<1>{source};", text)
    if found != 1:
        raise ValueError(f"expected one initial literal for v{reg}, found {found}")
    return text


def instrument(text,count):
    marker="[numthreads(1, 1, 1)]"
    if text.count(marker)!=1:
        raise ValueError("unexpected compute entry")
    text=text.replace(marker,"RWStructuredBuffer<uint> output : register(u0);\n"+marker)
    for j,v in enumerate(INIT):
        text=load_input(text,4+j,v,f"output[{STRIDE}u * gid.x + {j}u]")
    suffix="\n".join(f"    output[{STRIDE}u * gid.x + {3+j}u] = v{7+j};" for j in range(count))
    if not text.rstrip().endswith("}"):
        raise ValueError("missing main ending")
    return text.rstrip()[:-1]+suffix+"\n}\n"

def main():
    ap=argparse.ArgumentParser(description=__doc__)
    for n in ("emitter","llvm-mc","dxc","dxv","provider","proton","prefix","compiler-dir","work","output"):
        ap.add_argument("--"+n,type=Path,required=True)
    ap.add_argument("--baseline",type=Path)
    ap.add_argument("--clamp",action="store_true",help="test seven saturating arithmetic forms")
    ap.add_argument("--adapter",required=True)
    a=ap.parse_args()
    if a.output.exists():
        ap.error("choose a fresh receipt")
    a.work.mkdir(parents=True,exist_ok=False)
    rng=random.Random(0x5041434b)
    tool_hashes={n:digest(getattr(a,n)) for n in ("emitter","dxc","dxv","provider")}
    if a.baseline:tool_hashes["baseline"]=digest(a.baseline)
    ops=CLAMP_OPS if a.clamp else OPS
    rows=[]
    negatives=[]
    with TemporaryDirectory(dir=a.work) as td:
        trial=Path(td)
        guest=ROOT/"tools/reference/independent-shaders/compute.cpp"
        (trial/"guest.cpp").write_text(guest.read_text())
        (trial/"context.h").write_text((guest.parent/"context.h").read_text().replace('"../compute_buffers.h"','"'+str(ROOT/"tools/reference/compute_buffers.h")+'"'))
        expected={}
        for name,n in ops.items():
            variants=[(lo,hi) for lo in range(1<<n) for hi in range(1<<n)]
            for start in range(0,len(variants),8):
                chunk=variants[start:start+8]
                lines=[f"v_mov_b32_e32 v{4+j}, 0x{v:x}" for j,v in enumerate(INIT)]
                for j,(lo,hi) in enumerate(chunk):
                    lines+=variant(name,n,lo,hi,7+j,(start+j)%n,a.clamp)
                lines+=["s_endpgm"]
                words=assemble(lines,a.llvm_mc)
                if a.baseline:
                    before=translate(a.baseline,words)
                    if before.returncode!=65 or name not in before.stderr:
                        raise ValueError(f"baseline did not refuse {name}: {before.returncode} {before.stderr}")
                got=translate(a.emitter,words)
                got.check_returncode()
                src=instrument(got.stdout,len(chunk))
                label=f"packed{len(rows):02}"
                shader=trial/(label+".hlsl");shader.write_text(src)
                blob=shader.with_suffix(".cso")
                subprocess.run([a.dxc,"-T","cs_6_0","-E","main","-O3","-Fo",blob,shader],check=True,capture_output=True,timeout=30)
                subprocess.run([a.dxv,blob],check=True,capture_output=True,timeout=15)
                seed=[rng.getrandbits(32) for _ in range(384)]
                want=seed.copy()
                edge=[0,1,0xffff,0x10000,0x7fff8000,0x80007fff,0xffff0001,0xffffffff]
                for lane in range(GROUPS):
                    vals=[edge[(lane+j*3)%len(edge)] for j in range(3)] if lane<8 else [rng.getrandbits(32) for _ in range(3)]
                    if a.clamp and lane in (8,9,10,11):
                        vals=[[0xffffffff]*3,[0x80008000]*3,[0x7fff7fff]*3,[0,0xffffffff,0]][lane-8]
                    offset=96+STRIDE*lane
                    seed[offset:offset+3]=vals
                    want[offset:offset+3]=vals
                    want[offset+3:offset+3+len(chunk)]=[reference(name,vals,lo,hi,a.clamp) for lo,hi in chunk]
                (trial/(label+".seed")).write_bytes(struct.pack("<384I",*seed))
                expected[label]=struct.pack("<384I",*want)
                rows.append({"case":label,"opcode":name,"selectors":chunk,
                             "alias_indices":[(start+j)%n for j in range(len(chunk))],
                             "assembly":lines,"native_words":[f"{v:08x}" for v in words],
                             "emitted_sha256":hashlib.sha256(got.stdout.encode()).hexdigest(),
                             "instrumented_sha256":digest(shader),"dxil_sha256":digest(blob)})
            # Clamp and non-ordinary sources remain unsupported by this patch.
            operands=", ".join(f"v{4+j}" for j in range(n))
            controls=[("scalar_source",f"{name} v7, s4, "+", ".join(f"v{4+j}" for j in range(1,n))+(" clamp" if a.clamp else ""))]
            if not a.clamp and name in ("v_pk_max_u16","v_pk_min_u16"):
                controls.append(("clamp",f"{name} v7, {operands} clamp"))
            for kind,inst in controls:
                words=assemble([inst,"s_endpgm"],a.llvm_mc)
                bad=translate(a.emitter,words)
                if bad.returncode!=65:
                    raise ValueError(f"unexpected acceptance {inst}")
                negatives.append({"opcode":name,"kind":kind,"assembly":inst,"refusal":bad.stderr.strip()})
        (trial/"cases.txt").write_text("".join(f"{n} 4 structured {GROUPS}\n" for n in expected))
        exe,status,fields=run_gpu_guest(trial/"guest.cpp","packed.exe",trial,a.work,a.proton,a.prefix,a.provider,a.compiler_dir)
        if status or fields.get("adapter")!=a.adapter:
            raise RuntimeError(f"guest failed: {status} {fields}")
        for row in rows:
            name=row["case"];actual=trial/(name+".bin")
            observed=actual.read_bytes()
            if observed!=expected[name]:
                lhs,rhs=struct.unpack("<384I",observed),struct.unpack("<384I",expected[name])
                raise ValueError(f"{name} GPU mismatch {[(i,x,y) for i,(x,y) in enumerate(zip(lhs,rhs)) if x!=y][:8]}")
            row["readback_sha256"]=digest(actual)
        receipt={"scope":"Synthetic LLVM-assembled packed integer instructions; dynamic instrumented inputs and observed outputs, no game",
                 "adapter":fields["adapter"],"proton":(a.proton/"version").read_text().strip(),
                 "seed":0x5041434b,"clamped":a.clamp,"instructions":len(ops),"shader_cases":len(rows),
                 "selector_combinations":sum(len(r["selectors"]) for r in rows),
                 "result_words":GROUPS*sum(len(r["selectors"]) for r in rows),
                 "compared_words":384*len(rows),"mismatches":0,
                 "baseline_refused_shaders":len(rows) if a.baseline else None,
                 "negative_controls":negatives,"tool_sha256":tool_hashes,
                 "guest_sha256":digest(exe),"rows":rows}
    with a.output.open("x") as f:
        json.dump(receipt,f,indent=2);f.write("\n")
    print(f"PASS {receipt['selector_combinations']} selectors, {receipt['result_words']} results, {receipt['compared_words']} GPU words; zero mismatches")
if __name__=="__main__":
    main()
