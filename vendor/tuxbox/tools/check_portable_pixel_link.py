#!/usr/bin/env python3
"""Link a synthetic native VS to actual portable DXC pixel programs and read back."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import struct
import subprocess
import sys
from tempfile import TemporaryDirectory
ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT))
from tools.frosty_gpu_guest import run_gpu_guest
from tools.reference.portable_link_fixture import vertex

TYPES={1:"uint",2:"int",3:"float"}

def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

def expected():
    rgba=bytearray()
    depth=bytearray()
    for y in range(16):
        for x in range(16):
            inside=3<=x<12 and 2<=y<11
            rgba.extend(struct.pack("<4f",*((1.,.25,.5,1.) if inside else (.25,-.5,.75,1.5))))
            depth.extend(struct.pack("<f",.5 if inside else .875))
        depth.extend(bytes(192))
    return bytes(rgba+depth)

def run(argv=None):
    ap=argparse.ArgumentParser(description=__doc__)
    for name in ("emitter","provider","proton","prefix","compiler-dir","dxc","dxv","work","output"):
        ap.add_argument("--"+name,type=Path,required=True)
    ap.add_argument("--adapter",required=True)
    a=ap.parse_args(argv)
    if a.work.exists() or a.output.exists():ap.error("fresh work/output required")
    a.work.mkdir(parents=True)
    fingerprints={n:sha(getattr(a,n)) for n in ("emitter","provider","dxc","dxv")}
    rows=[]
    with TemporaryDirectory(dir=a.work) as tmp:
        trial=Path(tmp)
        for producer,ptype in TYPES.items():
            native=trial/f"vertex{producer}.dxbc"
            native.write_bytes(vertex(producer))
            for consumer,ctype in TYPES.items():
                name=f"p{producer}c{consumer}"
                ps=trial/(name+".hlsl")
                bits="v" if consumer==1 else "asuint(v)"
                ps.write_text(f"float4 main(nointerpolation {ctype}4 v:TEXCOORD0):SV_Target {{ uint4 b={bits}; return float4(b.x==0x89abcdefu?1:0,asfloat(b.yzw)); }}")
                pixel=trial/(name+".cso")
                subprocess.run([a.dxc,"-T","ps_6_0","-E","main","-O3","-Fo",pixel,ps],check=True,capture_output=True,timeout=30)
                subprocess.run([a.dxv,pixel],check=True,capture_output=True,timeout=30)
                vs=trial/("v"+name+".hlsl")
                profile=trial/("v"+name+".profile")
                subprocess.run([a.emitter,native,"--link-pixel",pixel,"--pixel-order",pixel,"--hlsl-out",vs,"--profile-out",profile],check=True,capture_output=True,timeout=30)
                text=vs.read_text()
                if not re.search(r"nointerpolation\s+"+ctype+r"4\s+o\d+\s*:\s*TEXCOORD0;",text):
                    raise ValueError(f"incorrect linked component type in {name}")
                blob=trial/("v"+name+".cso")
                subprocess.run([a.dxc,"-T",profile.read_text(),"-E","main","-O3","-Fo",blob,vs],check=True,capture_output=True,timeout=30)
                subprocess.run([a.dxv,blob],check=True,capture_output=True,timeout=30)
                rows.append(dict(case=name,producer=ptype,consumer=ctype,vertex_sha256=sha(blob),pixel_sha256=sha(pixel)))
        # A link cache entry must be specific to the actually bound pixel
        # binary. Reject a mismatched original/actual consumer before emit.
        wrong=trial/"wrong.hlsl"
        result=subprocess.run([a.emitter,trial/"vertex3.dxbc","--link-pixel",trial/"p3c1.cso",
            "--pixel-order",trial/"p3c2.cso","--hlsl-out",wrong],capture_output=True,timeout=30)
        if result.returncode!=65 or wrong.exists() or b"differs from its bound bytecode" not in result.stderr:
            raise ValueError("mismatched portable consumer was not refused")
        (trial/"cases.txt").write_text("".join(f"{row['case']} v{row['case']} 1 4\n" for row in rows))
        exe,status,fields=run_gpu_guest(ROOT/"tools/reference/independent-shaders/pixel.cpp",
                "portable-link.exe",trial,a.work,a.proton,a.prefix,a.provider,a.compiler_dir,
                extra_env={"TUXBOX_NULL_PIPELINES":"0"})
        want=expected()
        for row in rows:
            path=trial/(row["case"]+".bin")
            data=path.read_bytes() if path.is_file() else b""
            row["matched"]=data==want
            if data==want:row["readback_sha256"]=sha(path)
            else:row.update(error=fields.get(row["case"]+"_error"),bytes=len(data))
        unchanged=fingerprints=={n:sha(getattr(a,n)) for n in fingerprints}
        report=dict(schema=1,scope="Synthetic native vertex via production CLI, DXC portable pixel, desktop D3D12; not retail replay",
            wrong_consumer_rejected=True,passed=status==0 and fields.get("adapter")==a.adapter and unchanged and all(x["matched"] for x in rows),
            adapter=fields.get("adapter"),guest_exit=status,cases=len(rows),pixels=len(rows)*256,
            compared_components=len(rows)*256*5,tools_sha256=fingerprints,guest_sha256=sha(exe),rows=rows)
        if fields.get("error"):report["error"]=fields["error"]
        with a.output.open("x") as f:json.dump(report,f,indent=2);f.write("\n")
    print(report["passed"],report["cases"],report["pixels"],"failures",sum(not row["matched"] for row in rows))
    return int(not report["passed"])

if __name__=="__main__":
    raise SystemExit(run())
