#!/usr/bin/env python3
"""Differential GPU controls for native masked unsigned/float wave extrema."""
import argparse
import hashlib
import json
from pathlib import Path
import random
import re
import struct
import subprocess
import sys
from tempfile import TemporaryDirectory
ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT))
from tools.frosty_gpu_guest import run_gpu_guest

OPS=("umin","umax","fmin","fmax")
WORDS=384
START=96

def fbits(value):
    return struct.unpack("<I",struct.pack("<f",value))[0]

def fvalue(bits):
    return struct.unpack("<f",struct.pack("<I",bits))[0]

def inputs(width,op):
    rng=random.Random(0x45585452+width)
    full=(1<<width)-1
    masks=[full,0,1,1<<(width-1),full&0xaaaaaaaaaaaaaaaa,full&0x5555555555555555]
    if width==64:masks.extend((0xffffffff,0xffffffff00000000))
    while len(masks)<16:masks.append(rng.getrandbits(width))
    for i,mask in enumerate(masks):
        if op.startswith("f"):
            # Avoid NaNs, signed-zero ties and subnormals: those require
            # a separate ISA-policy oracle. Include exact negative/positive
            # finite values and both infinities.
            edge=[fbits(v) for v in (-float("inf"),-65536,-1,0,1,65536,float("inf"))]
            values=[edge[(j+i)%len(edge)] if i<6 else fbits(rng.randint(-100000,100000)) for j in range(width)]
        else:
            edge=[0,1,0x7fffffff,0x80000000,0xfffffffe,0xffffffff]
            values=[edge[(j+i)%len(edge)] if i<6 else rng.getrandbits(32) for j in range(width)]
        yield i,mask,values

def expected(width,op,mask,values):
    active=[(mask>>i)&1 for i in range(width)]
    selected=[v for v,a in zip(values,active) if a]
    if op=="umin":result=min(selected,default=0xffffffff)
    elif op=="umax":result=max(selected,default=0)
    elif op=="fmin":result=fbits(min(map(fvalue,selected),default=float("inf")))
    else:result=fbits(max(map(fvalue,selected),default=-float("inf")))
    seed=[0x76540000+i for i in range(WORDS)]
    seed[START:START+width]=values
    seed[START+width:START+2*width]=active
    want=seed.copy();want[START+2*width:START+3*width]=[result]*width
    return struct.pack("<384I",*seed),struct.pack("<384I",*want)

def instrument(hlsl,width):
    initial="    v0 = gtid.x; v1 = gtid.y; v2 = gtid.z;"
    ends=re.findall(r"    // 0x[0-9a-f]+ s_endpgm",hlsl)
    if hlsl.count(initial)!=1 or len(ends)!=1:
        raise ValueError("unexpected extrema fixture")
    hlsl=hlsl.replace(initial,f"    v0 = output[gtid.x]; v1 = gtid.y; v2 = gtid.z;\n    exec_lo = output[{width}u + gtid.x] & 1u;")
    hlsl=hlsl.replace(ends[0],f"    output[{2*width}u + gtid.x] = s7;\n"+ends[0])
    return "RWBuffer<uint> output : register(u0);\n"+hlsl

def digest(p):
    return hashlib.sha256(p.read_bytes()).hexdigest()

def main(argv=None):
    ap=argparse.ArgumentParser(description=__doc__)
    for n in ("emitter","provider","proton","prefix","compiler-dir","dxc","dxv","work","output"):
        ap.add_argument("--"+n,type=Path,required=True)
    ap.add_argument("--adapter",required=True)
    ap.add_argument("--width",type=int,choices=(32,64),required=True)
    a=ap.parse_args(argv)
    if a.work.exists() or a.output.exists():ap.error("fresh work/output required")
    a.work.mkdir(parents=True)
    with TemporaryDirectory(dir=a.work) as tmp:
        trial=Path(tmp);want={};shaders={}
        for op in OPS:
            emitted=subprocess.check_output([a.emitter,str(a.width),op],text=True,timeout=15)
            source=trial/(op+".hlsl");source.write_text(instrument(emitted,a.width))
            blob=trial/(op+".cso")
            subprocess.run([a.dxc,"-T","cs_6_6" if a.width==32 else "cs_6_0","-E","main","-Fo",blob,source],check=True,timeout=30)
            subprocess.run([a.dxv,blob],check=True,timeout=30)
            shaders[op]=digest(blob)
            for i,mask,values in inputs(a.width,op):
                name=f"{op}{i:02}"
                seed,want[name]=expected(a.width,op,mask,values)
                (trial/(name+".seed")).write_bytes(seed)
                (trial/(name+".cso")).hardlink_to(blob)
        (trial/"cases.txt").write_text("".join(f"{n} 0 uint 1\n" for n in want))
        exe,status,fields=run_gpu_guest(ROOT/"tools/reference/independent-shaders/compute.cpp",
            "wave-extrema.exe",trial,a.work,a.proton,a.prefix,a.provider,a.compiler_dir,
            extra_env={"TUXBOX_NULL_PIPELINES":"0"})
        rows=[]
        for name,reference in want.items():
            p=trial/(name+".bin")
            data=p.read_bytes() if p.is_file() else b""
            row={"case":name,"matched":data==reference}
            if data!=reference:
                row.update(expected=reference.hex(),actual=data.hex())
            rows.append(row)
        report=dict(schema=1,scope="Native masked wave extrema translated to HLSL; finite values and infinities; no NaN or signed-zero tie claims",
            passed=status==0 and fields.get("adapter")==a.adapter and all(row["matched"] for row in rows),
            adapter=fields.get("adapter"),width=a.width,guest_exit=status,cases=len(rows),compared_words=len(rows)*WORDS,
            result_words=len(rows)*a.width,emitter_sha256=digest(a.emitter),dxc_sha256=digest(a.dxc),
            dxv_sha256=digest(a.dxv),provider_sha256=digest(a.provider),guest_sha256=digest(exe),
            shaders=shaders,rows=rows)
        if fields.get("error"):report["error"]=fields["error"]
        with a.output.open("x") as stream:json.dump(report,stream,indent=2);stream.write("\n")
    print(report["passed"],report["cases"],report["compared_words"],"failed",sum(not row["matched"] for row in rows))
    return int(not report["passed"])
if __name__=="__main__":
    raise SystemExit(main())
