#!/usr/bin/env python3
"""Sample one explicit Linux PID, or analyze its recorded memory series.

No process discovery, signals, GPU probes, memory writes, or system tuning.
PSS apportions shared pages; RSS does not. Growth is evidence, not proof of a
leak. Process descendants and driver/VRAM allocations are not included.
"""
import argparse
import hashlib
import json
import math
import re
from pathlib import Path
from statistics import median
import time

FIELDS = ("Rss", "Pss", "Private_Clean", "Private_Dirty", "Swap", "SwapPss")
MAX_SAMPLES = 100000


def birth(proc):
    raw = (proc / "stat").read_bytes()
    return int(raw[raw.rindex(b")")+2:].split()[19])


def kib_fields(path, names):
    result = {}
    with path.open() as stream:
        for line in stream:
            key, sep, value = line.partition(":")
            if sep and key in names:
                fields = value.split()
                if len(fields) != 2 or fields[1] != "kB" or not fields[0].isdigit() or key in result:
                    raise ValueError("invalid or duplicate memory counter: " + key)
                result[key] = int(fields[0])
    return result


def snapshot(proc, expected_birth):
    if birth(proc) != expected_birth:
        raise ProcessLookupError("PID identity changed")
    fields = kib_fields(proc / "smaps_rollup", FIELDS)
    if set(fields) != set(FIELDS):
        raise ValueError("smaps_rollup missing required counters")
    if birth(proc) != expected_birth:
        raise ProcessLookupError("PID identity changed while sampling")
    return {"rss_kib": fields["Rss"], "pss_kib": fields["Pss"],
            "private_resident_kib": fields["Private_Clean"]+fields["Private_Dirty"],
            "swap_kib": fields["Swap"], "swap_pss_kib": fields["SwapPss"],
            "pss_plus_swap_pss_kib": fields["Pss"]+fields["SwapPss"]}


def sample(pid, seconds, interval, output):
    if pid <= 0 or not 0 < seconds <= 3600 or not .1 <= interval <= 60:
        raise ValueError("PID > 0, seconds 0..3600, interval 0.1..60")
    proc = Path("/proc") / str(pid)
    identity = birth(proc)
    start = time.monotonic()
    with output.open("x") as stream:
        stream.write(json.dumps({"schema":1,"pid":pid,"start_ticks":identity,
                                 "interval_seconds":interval})+"\n")
        while True:
            try:
                row = snapshot(proc, identity)
            except (FileNotFoundError, ProcessLookupError):
                stream.write(json.dumps({"end":"process_disappeared_or_changed"})+"\n")
                return
            elapsed = time.monotonic()-start
            stream.write(json.dumps({"seconds":elapsed,**row})+"\n")
            stream.flush()
            if elapsed >= seconds:
                stream.write(json.dumps({"end":"duration_reached"})+"\n")
                return
            time.sleep(min(interval,seconds-elapsed))


METRICS = ("rss_kib","pss_kib","private_resident_kib","swap_kib","swap_pss_kib","pss_plus_swap_pss_kib")


def summarize(path, growth_kib=16384, min_seconds=30):
    if not math.isfinite(min_seconds) or min_seconds <= 0 or growth_kib < 1:
        raise ValueError("invalid growth thresholds")
    rows = []
    end = None
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for i,raw in enumerate(iter(lambda:stream.readline(4097),b"")):
            if i > MAX_SAMPLES+1 or len(raw)>4096:
                raise ValueError("memory capture exceeds limits")
            digest.update(raw)
            data=json.loads(raw)
            if not isinstance(data,dict):
                raise ValueError("expected JSON object")
            if i==0:
                if data.get("schema")!=1 or any(type(data.get(k)) is not int or data[k]<1 for k in ("pid","start_ticks")):
                    raise ValueError("invalid process identity")
                interval=data.get("interval_seconds")
                if type(interval) not in (int,float) or not math.isfinite(interval) or not .1<=interval<=60:
                    raise ValueError("invalid sample interval")
                header=data
                continue
            if end is not None:
                raise ValueError("data after terminal record")
            if "end" in data:
                end=data["end"]
                if end not in ("duration_reached","process_disappeared_or_changed"):
                    raise ValueError("unknown terminal record")
                continue
            t=data.get("seconds")
            if type(t) not in (int,float) or not math.isfinite(t) or t<0 or (rows and t<=rows[-1]["seconds"]):
                raise ValueError("non-increasing or invalid sample time")
            if any(type(data.get(k)) is not int or data[k]<0 for k in METRICS):
                raise ValueError("invalid memory counter")
            if data["pss_plus_swap_pss_kib"]!=data["pss_kib"]+data["swap_pss_kib"]:
                raise ValueError("inconsistent PSS plus swap PSS")
            rows.append(data)
    if not rows:
        raise ValueError("no complete memory samples")
    duration=rows[-1]["seconds"]-rows[0]["seconds"]
    # Disjoint first and last quarters; avoid a single peak implying growth.
    n=max(1,len(rows)//4)
    stats={}
    for name in METRICS:
        values=[r[name] for r in rows]
        first,last=median(values[:n]),median(values[-n:])
        delta=last-first
        stats[name]={"first_median":first,"last_median":last,"delta":delta,"peak":max(values),
                     "peak_seconds":rows[values.index(max(values))]["seconds"],
                     "min":min(values),"last":values[-1]}
    # PSS+SwapPss avoids treating paging out private memory as a release.
    # Require multiple windows and observed retention, not monotonicity.
    values=[r["pss_plus_swap_pss_kib"] for r in rows]
    baseline=stats["pss_plus_swap_pss_kib"]["first_median"]
    tail_fraction=sum(v>=baseline+growth_kib for v in values[-n:])/n
    max_gap=max((b["seconds"]-a["seconds"] for a,b in zip(rows,rows[1:])),default=0)
    enough=len(rows)>=12 and duration>=min_seconds and end=="duration_reached" and max_gap<=3*header["interval_seconds"]
    return {"schema":1,"scope":"One Linux process; sampled memory, not allocation ownership or leak proof",
            "capture_sha256":digest.hexdigest(),"samples":len(rows),"duration_seconds":duration,
            "capture_end":end or "missing_terminal_record","metrics_kib":stats,
            "interval_seconds":header["interval_seconds"],"max_gap_seconds":max_gap,
            "growth":{"metric":"pss_plus_swap_pss_kib","threshold_kib":growth_kib,
                      "minimum_seconds":min_seconds,"tail_fraction_above_threshold":tail_fraction,
                      "result":"insufficient_capture" if not enough else
                      "retained_growth_observed" if tail_fraction>=.75 else "threshold_not_observed"}}


def compare_reports(a,b,peak_budget_kib):
    if peak_budget_kib<0:
        raise ValueError("peak budget must be nonnegative")
    if any(r["capture_end"]!="duration_reached" or r["samples"]<12
           or r["duration_seconds"]<r["growth"]["minimum_seconds"] for r in (a,b)):
        raise ValueError("comparison requires complete captures with at least 12 samples")
    if (a["interval_seconds"]!=b["interval_seconds"] or
        abs(a["duration_seconds"]-b["duration_seconds"])>max(a["interval_seconds"]*2,.05*a["duration_seconds"]) or
        any(r["max_gap_seconds"]>3*r["interval_seconds"] for r in (a,b))):
        raise ValueError("comparison requires similar durations, equal intervals and no large gaps")
    metric="pss_plus_swap_pss_kib"
    delta=b["metrics_kib"][metric]["peak"]-a["metrics_kib"][metric]["peak"]
    return {"metric":metric,"peak_delta_kib":delta,"allowed_peak_increase_kib":peak_budget_kib,
            "passed":delta<=peak_budget_kib}


# Optional allocation trace from PR #51. These are size queries, not live
# allocations or ownership events: summing their bytes would be misleading.
TRACE_HEADER = re.compile(r"allocation trace (info|info2-item) caller=(0x[0-9a-f]+) count=(\d+) bytes=(\d+) align=(\d+) id=(\d+)")
TRACE_ITEM = re.compile(r"allocation trace id=(\d+) item(\d+):dim=(\d+) (\d+)x(\d+)x(\d+) mips=(\d+) format=(\d+) samples=(\d+):(\d+) layout=(0x[0-9a-f]+) flags=(0x[0-9a-f]+) align=(\d+)")
SESSION = re.compile(r"^\[TuxBox [^\]]+\] Game log: ")


def allocation_queries(path):
    sessions=[]
    records={}
    capped=False
    digest=hashlib.sha256()
    total=0
    def finish():
        if not records and not capped:
            return
        rows=[]
        for ident,record in sorted(records.items()):
            header=record.get("header")
            items=record["items"]
            complete=bool(header and not record["conflict"] and set(items)==set(range(min(header["count"],8))))
            rows.append({"id":ident,"header":header,"items":[items[k] for k in sorted(items)],
                         "conflict":record["conflict"],"complete_logged_detail":complete,
                         "invalid_size_reported":bool(header and header["bytes"]==(1<<64)-1)})
        sessions.append({"trace_capped":capped,"records":rows,
                         "invalid_size_queries":sum(r["invalid_size_reported"] for r in rows),
                         "incomplete_records":sum(not r["complete_logged_detail"] for r in rows)})
    with path.open("rb") as stream:
        for n,raw in enumerate(iter(lambda:stream.readline(65537),b""),1):
            total+=len(raw)
            if total>512*1024*1024 or len(raw)>65536:
                raise ValueError("allocation log exceeds supported size or line length")
            digest.update(raw)
            line=raw.decode("utf-8",errors="replace")
            if SESSION.match(line):
                finish()
                records={}
                capped=False
            if "TuxBox d3d12_xs: " not in line:
                continue
            detail=line.split("TuxBox d3d12_xs: ",1)[1]
            if detail.startswith("allocation trace capped at "):
                capped=True
            elif m:=TRACE_HEADER.match(detail):
                kind,caller,count,size,align,ident=m.groups()
                ident=int(ident)
                row={"kind":kind,"caller_rva":caller,"count":int(count),
                     "bytes":int(size),"alignment":int(align),"line":n}
                r=records.setdefault(ident,{"items":{},"conflict":False})
                if "header" in r:
                    r["conflict"]|=any(r["header"][k]!=row[k] for k in row if k!="line")
                else:
                    r["header"]=row
            elif m:=TRACE_ITEM.match(detail):
                ident,index,*values=m.groups()
                ident,index=int(ident),int(index)
                names=("dimension","width","height","depth_or_array","mips","format","samples","quality","layout","flags","alignment")
                row={k:int(v,16 if v.startswith("0x") else 10) for k,v in zip(names,values)}
                row.update(index=index,line=n)
                r=records.setdefault(ident,{"items":{},"conflict":False})
                if index in r["items"]:
                    r["conflict"]|=any(r["items"][index][k]!=row[k] for k in row if k!="line")
                else:
                    r["items"][index]=row
            if len(records)>10000:
                raise ValueError("too many allocation query IDs in one session")
    finish()
    return {"schema":1,"scope":"Logged allocation size queries only; no inference about allocated bytes, live ownership or leaks",
            "log_sha256":digest.hexdigest(),"sessions":sessions,
            "observed_queries":sum(len(s["records"]) for s in sessions),
            "invalid_size_queries":sum(s["invalid_size_queries"] for s in sessions)}

def main(argv=None):
    parser=argparse.ArgumentParser(description=__doc__)
    commands=parser.add_subparsers(dest="command",required=True)
    capture=commands.add_parser("capture")
    capture.add_argument("pid",type=int)
    capture.add_argument("--seconds",type=float,default=60)
    capture.add_argument("--interval",type=float,default=1)
    capture.add_argument("--output",type=Path,required=True)
    report=commands.add_parser("analyze")
    report.add_argument("capture",type=Path)
    report.add_argument("--json",type=Path,required=True)
    report.add_argument("--growth-kib",type=int,default=16384)
    report.add_argument("--min-seconds",type=float,default=30)
    report.add_argument("--baseline",type=Path)
    report.add_argument("--peak-budget-kib",type=int)
    allocations=commands.add_parser("allocations")
    allocations.add_argument("log",type=Path)
    allocations.add_argument("--json",type=Path,required=True)
    args=parser.parse_args(argv)
    try:
        if args.command=="capture":
            sample(args.pid,args.seconds,args.interval,args.output)
            return 0
        if args.command=="allocations":
            result=allocation_queries(args.log)
            with args.json.open("x") as stream:
                json.dump(result,stream,indent=2,allow_nan=False)
                stream.write("\n")
            print(f"{result['observed_queries']} observed query IDs; {result['invalid_size_queries']} invalid size results")
            return 0
        if (args.baseline is None)!=(args.peak_budget_kib is None):
            raise ValueError("baseline and peak budget must be provided together")
        result=summarize(args.capture,args.growth_kib,args.min_seconds)
        if args.baseline:
            result["baseline"]=summarize(args.baseline,args.growth_kib,args.min_seconds)
            result["comparison"]=compare_reports(result["baseline"],result,args.peak_budget_kib)
        with args.json.open("x") as stream:
            json.dump(result,stream,indent=2,allow_nan=False)
            stream.write("\n")
        print(result["growth"]["result"])
        return int(not result.get("comparison",{}).get("passed",True))
    except (OSError,ValueError,KeyError,OverflowError) as exc:
        parser.error(str(exc))


if __name__=="__main__":
    raise SystemExit(main())
