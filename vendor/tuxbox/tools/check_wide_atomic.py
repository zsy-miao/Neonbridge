#!/usr/bin/env python3
"""GPU control for the native two-word unsigned image maximum emitter.

Capture the HLSL body from:
  cargo test --manifest-path tools/shader-compiler/Cargo.toml --target x86_64-unknown-linux-gnu verified_atomic_does_not_depend_on_shader_name -- --nocapture
Save the lines between BEGIN_ATOMIC and END_ATOMIC to --body.
For packed-instruction NSA coordinates, capture the body from
verified_nsa_atomic_uses_the_encoded_second_coordinate and pass --non-sequential.
The existing visibility guest is reused with synthetic vertex/pixel inputs.
No retail shaders or game assets are required.
"""
from pathlib import Path
import argparse
import subprocess
import sys
import json
import hashlib
from tempfile import TemporaryDirectory

p = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(p))
from build import find_dxc
from frosty_gpu_guest import run_gpu_guest

parser = argparse.ArgumentParser(description=__doc__)
for name in ("body", "provider", "proton", "prefix", "compiler-dir", "work", "output"):
    parser.add_argument("--" + name, type=Path, required=True)
parser.add_argument("--adapter", required=True)
parser.add_argument("--non-sequential", action="store_true",
                    help="Provide y in v5 and a deliberately different value in v2.")
args = parser.parse_args()
w = args.work.resolve()
w.mkdir(parents=True, exist_ok=True)
dxc = Path(find_dxc(None)).resolve()
body = args.body.read_text().rstrip("\n")
proton = args.proton.resolve()
def digest(b):return hashlib.sha256(b).hexdigest()
with TemporaryDirectory(dir=w) as td:
 trial=Path(td);header=[];shaders=[]
 for index,mesh,clip in [(2989,False,False),(2853,True,False),(3050,True,True)]:
  inputs="nointerpolation uint id:TEXCOORD1; "
  if clip:inputs+="nointerpolation int4 rect:TEXCOORD2; "
  inputs+="float4 pos:SV_Position; "
  if mesh:inputs+="nointerpolation uint4 cluster:TEXCOORD7; "
  val="((cluster.x << 7) + 128u) | cluster.y" if mesh else "id"
  gate="uint(all(int2(pos.xy) >= rect.xy) && all(int2(pos.xy) < rect.zw))" if clip else "1u"
  cluster_decl="uint4 cluster=i.cluster;" if mesh else ""
  rect_decl="int4 rect=i.rect;" if clip else ""
  y = "0xffffffffu" if args.non_sequential else "(uint)pos.y"
  nsa_y = "uint v5 = (uint)pos.y;" if args.non_sequential else ""
  ps=f"RWTexture2D<uint64_t> uav0 : register(u0);\nstruct Input {{ {inputs} }};\nvoid main(Input i) {{\nfloat4 pos=i.pos; uint id=i.id; {cluster_decl} {rect_decl}\nuint exec_lo = {gate}; uint v1 = (uint)pos.x, v2 = {y}, v3 = {val}, v4 = asuint(pos.z); {nsa_y}\n{body}\n}}\n"
  source=trial/f"ps{index}.hlsl";source.write_text(ps)
  for stage,path,profile,defs in [("vs",p/"tools/reference/visibility/vertex.hlsl","vs_6_0",["-DMESH_INPUT=1"]*mesh+["-DCLIP_INPUT=1"]*clip),("ps",source,"ps_6_6",[])]:
   blob=trial/f"{stage}{index}.dxil"
   subprocess.run([str(dxc),*defs,"-T",profile,"-E","main","-Fo",str(blob),str(path)],check=True,timeout=30)
   subprocess.run([str(dxc.with_name("dxv")),str(blob)],check=True,timeout=30)
   header.append(f"const unsigned char {stage}{index}[] = {{"+",".join(map(str,blob.read_bytes()))+"};\n")
   shaders.append(dict(stage=stage,case=index,dxil_sha256=digest(blob.read_bytes())))
 (trial/"visibility_shaders.h").write_text("".join(header))
 guest=(p/"tools/reference/visibility/guest.cpp").read_text().replace('"../compute_buffers.h"','"'+str(p/"tools/reference/compute_buffers.h")+'"').replace('"visibility.txt"','"result.txt"')
 (trial/"guest.cpp").write_text(guest)
 exe,status,fields=run_gpu_guest(trial/"guest.cpp","atomic.exe",trial,w,proton,args.prefix.resolve(),args.provider.resolve(),args.compiler_dir.resolve())
 print(status,fields)
 assert status==0 and fields.get("adapter")==args.adapter and fields.get("typed_int64_atomics")=="1" and all(fields.get(f"entry_{i}_matched")=="247" for i in (2989,2853,3050))
 receipt=dict(scope="Synthetic native image_atomic_umax emitted instruction, controlled pixel inputs, three competing fragments, not retail shaders",adapter=fields["adapter"],proton=(proton/"version").read_text().strip(),cases=3,texels=741,mismatches=0,body_sha256=digest(body.encode()),guest_sha256=digest(exe.read_bytes()),shaders=shaders,readback=fields)
 args.output.parent.mkdir(parents=True, exist_ok=True)
 args.output.write_text(json.dumps(receipt,indent=2)+"\n")
 print("PASS 741 texels")
