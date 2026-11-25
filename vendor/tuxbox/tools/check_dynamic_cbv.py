#!/usr/bin/env python3
"""Observe finite CBV selection on GPU using the actual emitted HLSL."""
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

def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

def instrument(hlsl):
    start="    // 0x0000 s_and_b32"
    end="    // 0x0018 s_endpgm"
    if hlsl.count(start)!=1 or hlsl.count(end)!=1 or "tb_dsel16 == 0x4000020u" not in hlsl:
        raise ValueError("unexpected emitter output")
    # The synthetic input is deliberately dynamic. Do not replace either
    # constant-buffer declaration, the mask, the selector or the load.
    hlsl=hlsl.replace(start,"    s8 = gid.x;\n"+start)
    hlsl=hlsl.replace(end,"    output[gid.x] = s20;\n"+end)
    return "RWBuffer<uint> output : register(u0);\n"+hlsl

def main(argv=None):
    ap=argparse.ArgumentParser(description=__doc__)
    for arg in ("hlsl","dxc","dxv","provider","proton","prefix","compiler-dir","work","output"):
        ap.add_argument("--"+arg,type=Path,required=True)
    ap.add_argument("--adapter",required=True)
    a=ap.parse_args(argv)
    if a.output.exists() or a.work.exists():
        ap.error("fresh work and output required")
    a.work.mkdir(parents=True)
    with TemporaryDirectory(dir=a.work) as tmp:
        trial=Path(tmp)
        source=trial/"selector.hlsl";source.write_text(instrument(a.hlsl.read_text()))
        blob=trial/"selector.cso"
        subprocess.run([a.dxc,"-T","cs_6_0","-E","main","-O3","-Fo",blob,source],check=True,timeout=30)
        subprocess.run([a.dxv,blob],check=True,timeout=30)
        exe,status,fields=run_gpu_guest(ROOT/"tools/reference/independent-shaders/cbv_select.cpp",
            "cbv-select.exe",trial,a.work,a.proton,a.prefix,a.provider,a.compiler_dir)
        want=[0x76540000+i for i in range(384)]
        want[96:224]=[0x12345678 if i%2==0 else 0xfedcba98 for i in range(128)]
        path=trial/"selector.bin"
        match=path.is_file() and path.read_bytes()==struct.pack("<384I",*want)
        result=dict(schema=1,scope="Synthetic dynamic HLSL selector, real CBV binding on GPU, injected gid input/output only",
                    passed=status==0 and fields.get("adapter")==a.adapter and match,
                    adapter=fields.get("adapter"),guest_exit=status,result_words=128,compared_words=384,
                    hlsl_sha256=digest(a.hlsl),dxil_sha256=digest(blob),provider_sha256=digest(a.provider),
                    guest_sha256=digest(exe),dxc_sha256=digest(a.dxc),dxv_sha256=digest(a.dxv))
        with a.output.open("x") as out:json.dump(result,out,indent=2);out.write("\n")
    print(result)
    return int(not result["passed"])
if __name__=="__main__":
    raise SystemExit(main())
