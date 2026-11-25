#!/usr/bin/env python3
"""Physical direct texture/sampler heap test for native pixel translations."""
import argparse
import hashlib
import json
from pathlib import Path
import struct
import subprocess
import sys
from tempfile import TemporaryDirectory
ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT))
from tools.frosty_gpu_guest import run_gpu_guest

def sha(p):
    return hashlib.sha256(p.read_bytes()).hexdigest()

def expected(texture,sampler):
    # Point at 0.375 selects upper-left. Linear uses weights 9/16,
    # 3/16, 3/16, 1/16. Texture 1 adds 2 to RGB independently.
    color=(0.625,0.25,0.25,1.) if sampler else (1.,0.,0.,1.)
    color=tuple(v+(2*texture if c<3 else 0) for c,v in enumerate(color))
    return b"".join(struct.pack("<4f",*(color if 3<=x<12 and 2<=y<11 else (-1.,-2.,-3.,-4.)))
                    for y in range(16) for x in range(16))

def main(argv=None):
    ap=argparse.ArgumentParser(description=__doc__)
    for name in ("emitter","provider","proton","prefix","compiler-dir","dxc","dxv","work","output"):
        ap.add_argument("--"+name,type=Path,required=True)
    ap.add_argument("--adapter",required=True)
    a=ap.parse_args(argv)
    if a.work.exists() or a.output.exists():ap.error("fresh work/output required")
    a.work.mkdir(parents=True)
    fingerprints={n:sha(getattr(a,n)) for n in ("emitter","provider","dxc","dxv")}
    rows=[];shaders={}
    with TemporaryDirectory(dir=a.work) as tmp:
        trial=Path(tmp)
        sources={"vertex":"float4 main(uint id:SV_VertexID):SV_Position { return float4(id==2?3:-1,id==1?3:-1,0.5,1); }"}
        for v in ("scalar","copy","inplace","branch","loop"):
            sources[v]=subprocess.check_output([a.emitter,v],text=True,timeout=15)
        for name,source in sources.items():
            hlsl=trial/(name+".hlsl");hlsl.write_text(source)
            blob=trial/(name+".cso")
            subprocess.run([a.dxc,"-T","vs_6_0" if name=="vertex" else "ps_6_6","-E","main","-O3","-Fo",blob,hlsl],check=True,timeout=30)
            subprocess.run([a.dxv,blob],check=True,timeout=30)
            shaders[name]=sha(blob)
        cases=[(f"{variant}_{tex}_{samp}",variant,tex,samp) for variant in ("scalar","copy","inplace","branch","loop") for tex in (0,1) for samp in (0,1)]
        (trial/"cases.txt").write_text("".join(f"{n} {v} {t} {s}\n" for n,v,t,s in cases))
        exe,status,fields=run_gpu_guest(ROOT/"tools/reference/independent-shaders/heap_offsets.cpp",
                "pixel-heap-offsets.exe",trial,a.work,a.proton,a.prefix,a.provider,a.compiler_dir,
                extra_env={"TUXBOX_NULL_PIPELINES":"0"})
        for n,v,t,s in cases:
            want=expected(1-t if v=="loop" else s if v=="branch" and s else t,s);path=trial/(n+".bin")
            data=path.read_bytes() if path.is_file() else b""
            row=dict(case=n,texture=t,sampler=s,matched=data==want)
            if data==want:row["readback_sha256"]=sha(path)
            else:row.update(expected_sha256=hashlib.sha256(want).hexdigest(),actual_sha256=hashlib.sha256(data).hexdigest(),
                            first_actual_pixel=list(struct.unpack_from("<4f",data,(2*16+3)*16)) if len(data)==4096 else None)
            rows.append(row)
        unchanged=fingerprints=={n:sha(getattr(a,n)) for n in fingerprints}
        report=dict(schema=1,scope="Synthetic native pixel HLSL, desktop D3D12 direct resource/sampler heap, no retail replay",
            passed=status==0 and fields.get("adapter")==a.adapter and unchanged and all(x["matched"] for x in rows),
            adapter=fields.get("adapter"),guest_exit=status,cases=len(rows),pixels=len(rows)*256,
            color_components=len(rows)*1024,tools_sha256=fingerprints,guest_sha256=sha(exe),shaders=shaders,rows=rows)
        if fields.get("error"):report["error"]=fields["error"]
        with a.output.open("x") as f:json.dump(report,f,indent=2);f.write("\n")
    print(report["passed"],report["cases"],report["pixels"],"failures",sum(not row["matched"] for row in rows),report.get("error",""))
    return int(not report["passed"])
if __name__=="__main__":
    raise SystemExit(main())
