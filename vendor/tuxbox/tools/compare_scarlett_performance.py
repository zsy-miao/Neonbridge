#!/usr/bin/env python3
"""Offline paired performance regression gate for recorded, equivalent runs.

Reuse TuxBox's MangoHud parser. Explicit run metadata and budgets are
required; no game launch, GPU probes, cache clearing or system tuning.
"""
import argparse
from collections import Counter
import hashlib
import json
import math
from pathlib import Path
from statistics import median
import sys

ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT))
from project_tuxbox.performance import read_frame_times
from project_tuxbox.pipeline_profile import summarize as summarize_pipelines

MAX_FILE=128*1024*1024
METRICS=("mean_fps","p95_ms","p99_ms","one_percent_low_fps","stutter_fraction")


def fingerprint(path):
    if not path.is_file() or path.stat().st_size>MAX_FILE:
        raise ValueError("capture missing or exceeds 128 MiB")
    with path.open("rb") as stream:
        digest=hashlib.sha256()
        for chunk in iter(lambda:stream.read(65536),b""):
            digest.update(chunk)
    return digest.hexdigest()


def finite(value, minimum=0):
    return type(value) in (int,float) and math.isfinite(value) and value>=minimum


def frame_summary(path,seconds,stutter_ms,min_frames,max_skipped_fraction):
    digest=fingerprint(path)
    times,skipped=read_frame_times(path,seconds)
    if fingerprint(path)!=digest:
        raise ValueError("frame capture changed during analysis")
    n=len(times)
    if n<min_frames or skipped/(n+skipped)>max_skipped_fraction:
        raise ValueError("insufficient valid frames or too many skipped rows")
    times.sort()
    total=math.fsum(times)
    # A single swapchain's per-present durations should not exceed the
    # entire recorded lifetime; reject mixed/duplicated capture evidence.
    if total>seconds*1000*1.01:
        raise ValueError("frame durations exceed declared run lifetime")
    tail=times[-math.ceil(n*.01):]
    stutters=sum(v>stutter_ms for v in times)
    return {"sha256":digest,"frames":n,"skipped_rows":skipped,
            "sampled_seconds":total/1000,"mean_fps":1000*n/total,
            "p50_ms":times[math.ceil(n*.50)-1],
            "p95_ms":times[math.ceil(n*.95)-1],"p99_ms":times[math.ceil(n*.99)-1],
            "max_ms":times[-1],"one_percent_low_fps":1000*len(tail)/math.fsum(tail),
            "stutter_frames":stutters,"stutter_fraction":stutters/n}


def cpu_summary(path):
    digest=fingerprint(path)
    seconds=busy=host_sum=host_seconds=0.0
    pid=None
    last=None
    threads=Counter()
    with path.open() as stream:
        for line in stream:
            row=json.loads(line)
            if not isinstance(row,dict):
                raise ValueError("invalid CPU interval")
            p,t,d,c,h=(row.get(k) for k in ("pid","end_unix_ns","seconds","process_one_core_percent","host_busy_percent"))
            hz=row.get("clock_ticks_per_second")
            if (type(p) is not int or p<=0 or (pid is not None and p!=pid)
                or type(t) is not int or (last is not None and t<=last)
                or not finite(d) or d<=0 or not finite(c)
                or h is not None and (not finite(h) or h>100)
                or type(hz) is not int or hz<=0):
                raise ValueError("invalid CPU interval or mixed process capture")
            pid,last=p,t
            seconds+=d
            busy+=c*d
            if h is not None:
                host_sum+=h*d
                host_seconds+=d
            for thread in row["threads"]:
                if (not isinstance(thread.get("name"),str)
                    or type(thread.get("cpu_ticks")) is not int or thread["cpu_ticks"]<0):
                    raise ValueError("invalid thread counters")
                threads[thread["name"]]+=thread["cpu_ticks"]/hz
    if not seconds or fingerprint(path)!=digest:
        raise ValueError("empty or changing CPU capture")
    return {"sha256":digest,"sampled_seconds":seconds,
            "process_mean_one_core_percent":busy/seconds,
            "host_mean_busy_percent":host_sum/host_seconds if host_seconds else None,
            "top_observed_thread_names_cpu_seconds":dict(threads.most_common(10)),
            "scope":"One process; thread accounting excludes threads not seen at both interval boundaries."}


def pipeline_summary(path):
    digest=fingerprint(path)
    report=summarize_pipelines(path)
    if fingerprint(path)!=digest or not report["groups"]:
        raise ValueError("empty or changing pipeline trace")
    if any(not finite(v["total_ms"]) for v in report["groups"].values()):
        raise ValueError("invalid pipeline totals")
    return {"sha256":digest,**report,
            "scope":"Sum of recorded PSO call durations; concurrent operations may overlap, not wall time."}


def check(manifest,root):
    if not isinstance(manifest,dict) or manifest.get("schema")!=1:
        raise ValueError("invalid manifest schema")
    context=manifest.get("context")
    # Explicitly record the comparison contract. We cannot verify truth of
    # the operator's declarations from a CSV.
    keys=("scenario","hardware","driver","backend","resolution","settings","cache_state")
    if not isinstance(context,dict) or any(not isinstance(context.get(k),str) or not context[k].strip() for k in keys):
        raise ValueError("declare scenario, hardware, driver, backend, resolution, settings and cache_state")
    budget=manifest.get("budgets")
    required=("mean_fps_drop_pct","p95_increase_pct","p99_increase_pct","one_percent_low_drop_pct","stutter_fraction_increase")
    if (not isinstance(budget,dict) or set(budget)!=set(required)
        or any(not finite(v) for v in budget.values())
        or budget["stutter_fraction_increase"]>1):
        raise ValueError("explicit nonnegative budgets required for every metric")
    stutter=manifest.get("stutter_ms")
    min_frames=manifest.get("min_frames",300)
    skipped=manifest.get("max_skipped_fraction",.05)
    if not finite(stutter) or stutter<=0 or type(min_frames) is not int or min_frames<100 or not finite(skipped) or skipped>1:
        raise ValueError("invalid sample policy")
    pairs=manifest.get("pairs")
    if not isinstance(pairs,list) or not 3<=len(pairs)<=30:
        raise ValueError("require 3..30 paired runs")
    seen=set()
    seen_hashes={"baseline":set(),"candidate":set()}
    results=[]
    def read_run(row):
        if not isinstance(row,dict) or not finite(row.get("seconds")) or row["seconds"]<=0:
            raise ValueError("declare positive run lifetime in seconds")
        if not isinstance(row.get("frames"),str) or not row["frames"]:
            raise ValueError("frame capture path required")
        for optional in ("cpu","pipelines"):
            if optional in row and (not isinstance(row[optional],str) or not row[optional]):
                raise ValueError("invalid optional capture path")
        path=(root/row["frames"]).resolve()
        if path in seen:
            raise ValueError("do not reuse frame capture paths as independent runs")
        seen.add(path)
        result=frame_summary(path,row["seconds"],stutter,min_frames,skipped)
        if row.get("cpu"):
            result["cpu"]=cpu_summary((root/row["cpu"]).resolve())
        if row.get("pipelines"):
            result["pipelines"]=pipeline_summary((root/row["pipelines"]).resolve())
        return result
    for i,pair in enumerate(pairs):
        if not isinstance(pair,dict) or set(pair)!={"baseline","candidate"}:
            raise ValueError("each pair must name baseline and candidate")
        a,b=(read_run(pair[side]) for side in ("baseline","candidate"))
        for side,row in (("baseline",a),("candidate",b)):
            if row["sha256"] in seen_hashes[side]:
                raise ValueError("duplicate frame capture bytes within one variant")
            seen_hashes[side].add(row["sha256"])
        if any((k in a)!=(k in b) for k in ("cpu","pipelines")):
            raise ValueError("optional diagnostic inputs must be paired")
        ratio=lambda key:100*(b[key]-a[key])/a[key]
        changes={"mean_fps_drop_pct":-ratio("mean_fps"),
                 "p95_increase_pct":ratio("p95_ms"),"p99_increase_pct":ratio("p99_ms"),
                 "one_percent_low_drop_pct":-ratio("one_percent_low_fps"),
                 "stutter_fraction_increase":b["stutter_fraction"]-a["stutter_fraction"]}
        violations=[k for k,v in changes.items() if v>budget[k]+1e-10]
        results.append({"pair":i+1,"baseline":a,"candidate":b,"changes":changes,"violations":violations})
    return {"schema":1,"scope":"Paired recorded frame-time comparisons; supplied conditions are declarations, not independently verified; no causal bottleneck claim",
            "context":context,"budgets":budget,"stutter_ms":stutter,"min_frames":min_frames,
            "max_skipped_fraction":skipped,"pairs":results,
            "median_changes":{k:median(r["changes"][k] for r in results) for k in required},
            "worst_changes":{k:max(r["changes"][k] for r in results) for k in required},
            "passed":all(not row["violations"] for row in results)}


def main(argv=None):
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("manifest",type=Path)
    parser.add_argument("--json",type=Path,required=True)
    args=parser.parse_args(argv)
    try:
        if args.json.exists():
            raise ValueError("choose a fresh report path")
        report=check(json.loads(args.manifest.read_text()),args.manifest.parent)
        with args.json.open("x") as stream:
            json.dump(report,stream,indent=2,allow_nan=False)
            stream.write("\n")
    except (OSError,ValueError,KeyError,TypeError,OverflowError) as exc:
        parser.error(str(exc))
    print(f"{sum(not r['violations'] for r in report['pairs'])}/{len(report['pairs'])} pairs within budgets")
    for key,val in report["median_changes"].items():
        print(f"  {key}: median {val:+.4f}, worst {report['worst_changes'][key]:+.4f}")
    return int(not report["passed"])


if __name__=="__main__":
    raise SystemExit(main())
