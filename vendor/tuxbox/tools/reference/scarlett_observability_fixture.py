#!/usr/bin/env python3
"""Reproduce offline diagnosis and 4K pixel-comparison controls."""
import argparse
import json
from pathlib import Path
import sys
from time import monotonic

ROOT=Path(__file__).resolve().parents[2]
sys.path.insert(0,str(ROOT))
from tools.diagnose_scarlett_run import analyze
from tools.compare_scarlett_frames import compare
from PIL import Image, ImageDraw


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory",type=Path)
    args=parser.parse_args()
    root=args.directory
    root.mkdir(parents=True,exist_ok=False)
    stamp="[TuxBox 2026-11-24T07:37:07+00:00] "
    log=root/"synthetic.log"
    log.write_text("\n".join([
        stamp+"Game log: synthetic",
        stamp+"Starting game",
        "TuxBox d3d12_xs: PresentX presented first frame (3840x2160)",
        "TuxBox d3d12_xs: continued past recoverable title assert RVA 0x4ffd879 (hit 1)",
        "TuxBox d3d12_xs: slot 86 pipeline stream (688 bytes) failed: 0x8007007e",
        "wine: Unhandled exception 0xc0000005 in thread 0040 at address 0000000140001234 (thread 0040), starting debugger...",
        stamp+"Game exited with 3 after 97.0s",
        stamp+"Game log: synthetic-second",
        stamp+"Starting game",
        "TuxBox d3d12_xs: created host D3D12 device (adapter LUID 00000000:00000000)",
        stamp+"Game exited with 0 after 1.0s",
    ])+"\n")
    diagnosis=analyze(log)
    assert [s["outcome"] for s in diagnosis["sessions"]]==["nonzero_exit","normal_exit"]
    assert diagnosis["sessions"][0]["investigation_order"][0]["last"]["kind"]=="unhandled_wine_exception"
    assert diagnosis["sessions"][1]["findings"]==[]
    (root/"diagnosis.json").write_text(json.dumps(diagnosis,indent=2)+"\n")

    size=(3840,2160)
    reference=Image.new("RGB",size,(32,64,96))
    draw=ImageDraw.Draw(reference)
    draw.rectangle((1920,0,3839,2159),fill=(128,160,192))
    draw.rectangle((640,480,1279,959),fill=(192,128,64))
    ref=root/"reference.png"
    reference.save(ref)
    reports={}
    for name in ("identity","one_pixel","local_artifact","black","tolerated","color_swap"):
        other=reference.copy()
        if name=="one_pixel":
            other.putpixel((3839,2159),(129,160,192))
        elif name=="local_artifact":
            ImageDraw.Draw(other).rectangle((64,64,79,79),fill=(255,64,96))
        elif name=="black":
            other=Image.new("RGB",size,(0,0,0))
        elif name=="tolerated":
            other=other.point(lambda value:min(value+1,255))
        elif name=="color_swap":
            r,g,b=other.split()
            other=Image.merge("RGB",(b,g,r))
        candidate=root/(name+".png")
        other.save(candidate)
        t0=monotonic()
        report=compare(ref,candidate,tolerance=1 if name=="tolerated" else 0,
                       max_fraction=.001 if name=="local_artifact" else 0,
                       max_tile_fraction=.01 if name=="local_artifact" else 0,
                       heatmap=root/(name+"-diff.png"))
        reports[name]={**report,"cpu_wall_seconds":round(monotonic()-t0,4)}
    assert reports["identity"]["passed"] and reports["identity"]["changed_pixels"]==0
    assert not reports["one_pixel"]["passed"] and reports["one_pixel"]["changed_pixels"]==1
    assert not reports["local_artifact"]["passed"] and reports["local_artifact"]["changed_pixels"]==256
    assert reports["local_artifact"]["over_tolerance_fraction"]<.001
    assert reports["local_artifact"]["max_tile_fraction"]==.0625
    assert not reports["black"]["passed"] and reports["black"]["candidate"]["near_black_fraction"]==1
    assert reports["tolerated"]["passed"] and reports["tolerated"]["changed_pixels"]==size[0]*size[1]
    assert not reports["color_swap"]["passed"]
    (root/"frames.json").write_text(json.dumps(reports,indent=2)+"\n")
    print("PASS six 4K frame pairs; exact, local, black, tolerated and channel errors")
    print("PASS independent launch boundaries, terminal exception and recovered assert")


if __name__=="__main__":
    main()
