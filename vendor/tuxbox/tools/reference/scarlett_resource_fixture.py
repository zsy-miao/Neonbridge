#!/usr/bin/env python3
"""Reproduce a real CPU memory allocation and synthetic performance gates."""
import argparse
import json
from itertools import accumulate
from pathlib import Path
import subprocess
import sys

ROOT=Path(__file__).resolve().parents[2]
sys.path.insert(0,str(ROOT))
from tools.scarlett_memory import sample, summarize, compare_reports
from tools.compare_scarlett_performance import check

# The only monitored process is this disposable Python child. Touch every
# page, so reserving virtual address space alone cannot pass the control.
CHILD = """
import mmap, sys, time
print("ready", flush=True)
time.sleep(.85)
mapping = None
if sys.argv[1] != "baseline":
    mapping = mmap.mmap(-1, 32*1024*1024)
    for i in range(0, len(mapping), 4096):
        mapping[i] = 1
    if sys.argv[1] == "released":
        time.sleep(.45)
        mapping.close()
time.sleep(4)
"""


def memory_control(root):
    reports={}
    for kind in ("baseline","retained","released"):
        path=root/(kind+".jsonl")
        with subprocess.Popen([sys.executable,"-c",CHILD,kind],stdout=subprocess.PIPE,
                              text=True) as child:
            try:
                if child.stdout.readline().strip()!="ready":
                    raise RuntimeError("memory control child failed to start")
                sample(child.pid,2.6,.1,path)
            finally:
                if child.poll() is None:
                    child.terminate()
                child.wait(timeout=5)
        reports[kind]=summarize(path,16384,2)
    assert reports["baseline"]["growth"]["result"]=="threshold_not_observed"
    assert reports["retained"]["growth"]["result"]=="retained_growth_observed"
    assert reports["released"]["growth"]["result"]=="threshold_not_observed"
    budget=compare_reports(reports["baseline"],reports["retained"],16384)
    assert not budget["passed"]
    assert reports["released"]["metrics_kib"]["pss_plus_swap_pss_kib"]["peak"]-reports["released"]["metrics_kib"]["pss_plus_swap_pss_kib"]["first_median"]>=16384
    return {"reports":reports,"retained_peak_gate":budget}


def csv_file(path,times,serial):
    path.write_text("fps,frametime,elapsed\n"+"".join(
        f"60,{v},{int(elapsed*1000000)+serial}\n" for v,elapsed in zip(times,accumulate(times))))


def performance_control(root):
    reports={}
    for kind in ("improved","tail_regression"):
        pairs=[]
        for i in range(3):
            a=root/f"{kind}-base{i}.csv"
            b=root/f"{kind}-new{i}.csv"
            csv_file(a,[10]*1000,i)
            times=[9]*1000
            if kind=="tail_regression" and i==1:
                times[-20:]=[100]*20
            csv_file(b,times,i)
            pairs.append({"baseline":{"frames":a.name,"seconds":20},
                          "candidate":{"frames":b.name,"seconds":20}})
        manifest={"schema":1,"context":{k:"synthetic-control" for k in
                  ("scenario","hardware","driver","backend","resolution","settings","cache_state")},
                  "budgets":{"mean_fps_drop_pct":5,"p95_increase_pct":10,
                             "p99_increase_pct":10,"one_percent_low_drop_pct":10,
                             "stutter_fraction_increase":.01},
                  "stutter_ms":50,"min_frames":100,"pairs":pairs}
        (root/(kind+"-manifest.json")).write_text(json.dumps(manifest,indent=2)+"\n")
        reports[kind]=check(manifest,root)
    assert reports["improved"]["passed"]
    bad=reports["tail_regression"]
    assert not bad["passed"] and bad["median_changes"]["mean_fps_drop_pct"]<0
    assert bad["pairs"][1]["violations"]==["mean_fps_drop_pct","p99_increase_pct","one_percent_low_drop_pct","stutter_fraction_increase"]
    return reports


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory",type=Path)
    args=parser.parse_args()
    root=args.directory
    root.mkdir(parents=True,exist_ok=False)
    report={"scope":"Real isolated Linux process memory and synthetic MangoHud CSV comparisons; no GPU or title execution",
            "memory":memory_control(root),"performance":performance_control(root)}
    (root/"receipt.json").write_text(json.dumps(report,indent=2,allow_nan=False)+"\n")
    print("PASS real 32 MiB memory retention/release and six performance pairs")


if __name__=="__main__":
    main()
