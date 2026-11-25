#!/usr/bin/env python3
"""Native DPP8 translation through DXC/DXV and a real guarded raw UAV."""
import argparse
import hashlib
import json
from pathlib import Path
import random
import struct
import subprocess
import sys
from tempfile import TemporaryDirectory
ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT))
from tools.frosty_gpu_guest import run_gpu_guest

def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

def signed(value):
    return value if value < 0x80000000 else value - 0x100000000

def reference(width, op, selectors, lane, bias, rhs):
    values=[]
    for i in range(width):
        source=(i//8)*8+selectors[i%8]
        a=source ^ bias
        value={"add":lambda:(a+rhs)&0xffffffff,
               "and":lambda:a&rhs,"or":lambda:a|rhs,"xor":lambda:a^rhs,
               "smin":lambda:min(a,rhs,key=signed),"smax":lambda:max(a,rhs,key=signed),
               "min":lambda:min(a,rhs),"max":lambda:max(a,rhs)}[op]()
        values.append(value)
    if lane>=0:
        values=[values[lane]]*width
    expected=[0x76540000+i for i in range(384)]
    expected[96:96+width]=values
    return struct.pack("<384I",*expected)

def cases():
    rng=random.Random(0x44505038)
    controls=[(1,0,3,2,5,4,7,6),(7,6,5,4,3,2,1,0),
              (0,0,0,0,7,7,7,7),tuple(rng.randrange(8) for _ in range(8))]
    for width,lane in ((32,-1),(32,31),(64,-1)):
        for op in ("add","and","or","min","max","smin","smax","xor"):
            for i,sel in enumerate(controls):
                bias=(0,0xfffffff0,0x80000000,0x55555555)[i]
                rhs=(16,0xfffffff0,0x7fffffff,0x5555555a)[i]
                for fi in (0,1):
                    yield width,op,sel,lane,bias,rhs,fi

def main(argv=None):
    ap=argparse.ArgumentParser(description=__doc__)
    for name in ("emitter","provider","proton","prefix","compiler-dir","dxc","dxv","work","output"):
        ap.add_argument("--"+name,type=Path,required=True)
    ap.add_argument("--adapter",required=True)
    a=ap.parse_args(argv)
    if a.work.exists() or a.output.exists():ap.error("fresh work and output required")
    a.work.mkdir(parents=True)
    fingerprints={n:sha(getattr(a,n)) for n in ("emitter","provider","dxc","dxv")}
    rows=[];expected={}
    with TemporaryDirectory(dir=a.work) as tmp:
        trial=Path(tmp)
        for i,(width,op,sel,lane,bias,rhs,fi) in enumerate(cases()):
            name=f"dpp{i:03}"
            control=sum(v<<(3*j) for j,v in enumerate(sel))
            source=subprocess.check_output([a.emitter,str(width),op,f"{control:x}",str(lane),f"{bias:x}",f"{rhs:x}",str(fi)],text=True,timeout=15)
            hlsl=trial/(name+".hlsl");hlsl.write_text(source)
            blob=trial/(name+".cso")
            subprocess.run([a.dxc,"-T","cs_6_0","-E","main","-O3","-Fo",blob,hlsl],check=True,capture_output=True,timeout=30)
            subprocess.run([a.dxv,blob],check=True,capture_output=True,timeout=30)
            expected[name]=reference(width,op,sel,lane,bias,rhs)
            rows.append(dict(case=name,width=width,operation=op,selectors=sel,lane=lane,bias=bias,rhs=rhs,fi=fi,dxil_sha256=sha(blob)))
        (trial/"cases.txt").write_text("".join(f"{n} 0 raw 1\n" for n in expected))
        exe,status,fields=run_gpu_guest(ROOT/"tools/reference/independent-shaders/compute.cpp",
                "dpp8-exchange.exe",trial,a.work,a.proton,a.prefix,a.provider,a.compiler_dir,
                extra_env={"TUXBOX_NULL_PIPELINES":"0"})
        for row in rows:
            name=row["case"];path=trial/(name+".bin")
            data=path.read_bytes() if path.is_file() else b""
            row["matched"]=data==expected[name]
            if row["matched"]:row["readback_sha256"]=sha(path)
            else:row.update(actual=data.hex(),expected=expected[name].hex())
        unchanged=fingerprints=={n:sha(getattr(a,n)) for n in fingerprints}
        result=dict(schema=1,scope="Native synthetic instructions, actual emitted HLSL and reflected raw UAV store; no output instrumentation",
                    adapter=fields.get("adapter"),guest_exit=status,passed=status==0 and fields.get("adapter")==a.adapter and unchanged and all(x["matched"] for x in rows),
                    cases=len(rows),compared_words=len(rows)*384,result_words=sum(x["width"] for x in rows),
                    tools_sha256=fingerprints,guest_sha256=sha(exe),rows=rows)
        if fields.get("error"):result["error"]=fields["error"]
        with a.output.open("x") as stream:json.dump(result,stream,indent=2);stream.write("\n")
    print(result["passed"],result["cases"],result["compared_words"],"failed",sum(not row["matched"] for row in rows))
    return int(not result["passed"])
if __name__=="__main__":
    raise SystemExit(main())
