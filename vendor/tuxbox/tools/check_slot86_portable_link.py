#!/usr/bin/env python3
"""Exercise real Scarlett slot 86 link requests, cold and persistent warm cache."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
from tempfile import TemporaryDirectory

ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT))
from tools.reference.portable_link_fixture import vertex
from tools.frosty_gpu_guest import run_gpu_guest
from project_tuxbox.shader_prep import SOURCE_CACHE

def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

def compiled_keys(log):
    return [line.removeprefix("Compiled Xbox shader ") for line in log.splitlines()
            if line.startswith("Compiled Xbox shader ")]

def cache_receipt(root, keys):
    directories=list(root.iterdir())
    if len(directories)!=1 or not directories[0].is_dir():
        raise ValueError("expected exactly one compiler fingerprint")
    cache=directories[0]
    result={}
    for key in keys:
        output=cache/(key+".cso")
        digest=sha(output)
        if output.with_suffix(".sha256").read_text()!=digest:
            raise ValueError("invalid persistent cache checksum")
        result[key]=digest
    if {p.stem for p in cache.glob("*.cso")}!=set(keys) or list(cache.glob("*.unsupported")):
        raise ValueError("unexpected cached shader or refusal")
    return result

def main(argv=None):
    ap=argparse.ArgumentParser(description=__doc__)
    for name in ("emitter","provider","proton","prefix","compiler-dir","dxc","dxv","work","output"):
        ap.add_argument("--"+name,type=Path,required=True)
    ap.add_argument("--adapter",required=True)
    a=ap.parse_args(argv)
    if a.work.exists() or a.output.exists():ap.error("fresh work and output required")
    if a.dxv.resolve()!=a.dxc.with_name("dxv").resolve():ap.error("worker requires sibling dxv")
    a.work.mkdir(parents=True)
    os.environ.update(TUXBOX_XSHADER=str(a.emitter.resolve()),DXC=str(a.dxc.resolve()))
    fingerprints={n:sha(getattr(a,n)) for n in ("emitter","provider","dxc","dxv")}
    runs={}
    with TemporaryDirectory(dir=a.work) as temp:
        workspace=Path(temp)
        for mode in ("cold","warm"):
            trial=workspace/mode;trial.mkdir()
            (trial/"vertex.dxbc").write_bytes(vertex(3))
            source=sha(trial/"vertex.dxbc")
            keys=[source]
            for i,kind in enumerate(("uint","int")):
                path=trial/f"p{i}.hlsl"
                swizzle="zyw" if i else "yzw"
                path.write_text(f"float4 main(nointerpolation {kind}4 v:TEXCOORD0):SV_Target {{ uint4 b=asuint(v); return float4(b.x==0x89abcdefu?1:0,asfloat(b.{swizzle})); }}")
                blob=trial/f"p{i}.cso"
                subprocess.run([a.dxc,"-T","ps_6_0","-E","main","-Fo",blob,path],check=True,timeout=30)
                subprocess.run([a.dxv,blob],check=True,timeout=30)
                digest=sha(blob)
                keys.append(f"{source}_{digest}_{digest}")
            log_dir=a.work/mode;log_dir.mkdir()
            exe,status,fields=run_gpu_guest(ROOT/"tools/reference/independent-shaders/slot86_link.cpp",
                "slot86-link.exe",trial,log_dir,a.proton,a.prefix,a.provider,a.compiler_dir,
                link_libraries=(str(a.provider.with_suffix(".lib")),),
                extra_env={"TUXBOX_NULL_PIPELINES":"0","TUXBOX_LOG":"stderr"},
                fresh_shader_cache=True,warm_shader_cache=workspace/"saved-cache" if mode=="warm" else None)
            log=(log_dir/"gpu.log").read_text(errors="replace")
            observed=compiled_keys(log)
            if status or fields!={"adapter":a.adapter,"draws":"4","pixels":"1024"}:
                raise ValueError(f"{mode} guest failed: {status} {fields}")
            if sorted(observed)!=sorted(keys if mode=="cold" else []):
                raise ValueError(f"unexpected {mode} compiler activity: {observed}")
            if "linked slot 86 VS=" not in log or "substituted null " in log:
                raise ValueError("missing real link or unexpected null fallback")
            cache=trial/"prefix"/SOURCE_CACHE.parent/"runtime"
            hashes=cache_receipt(cache,keys)
            if mode=="cold":
                shutil.copytree(cache,workspace/"saved-cache")
                cold_hashes=hashes
            elif hashes!=cold_hashes:
                raise ValueError("warm cache differs from cold")
            runs[mode]={"draws":4,"pixels":1024,"compiled":len(observed),"cached_programs":len(hashes),
                        "guest_sha256":sha(exe),"program_sha256":hashes}
        if fingerprints!={n:sha(getattr(a,n)) for n in fingerprints}:
            raise ValueError("tools changed during campaign")
    report={"schema":1,"passed":True,"adapter":a.adapter,"tools_sha256":fingerprints,"runs":runs,
            "scope":"synthetic native VS and portable PS, actual Scarlett slot 86 and worker, cold and warm processes"}
    with a.output.open("x") as f:json.dump(report,f,indent=2);f.write("\n")
    print("PASS: 8 draws, 2048 pixels, 3 cold compiles, 0 warm compiles")
if __name__=="__main__":
    main()
