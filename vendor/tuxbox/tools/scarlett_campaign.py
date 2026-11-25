#!/usr/bin/env python3
"""Bounded differential campaigns for the #60 ordinary scalar ISA subset.

Only synthetic straight-line, initialized integer programs. Reuse the
production bridge and CPU oracle; do not infer game coverage from this.
"""
import argparse
from collections import Counter, defaultdict
import hashlib
import json
import math
import os
from pathlib import Path
import random
import re
import subprocess
import sys
import tempfile
import time

sys.path.insert(0,str(Path(__file__).resolve().parent))
from reference.rdna2_scalar import decode, execute, encode, widths, END, MASK, FORMS, EXTENDED
from generate_scalar_tests import operation, BOUNDARIES
from check_scalar_oracle import check, digest

MAX_CASES=20000
MAX_REDUCTIONS=512
SCOPE="ordinary scalar synthetic RDNA2; no GPU, game data, branch coverage or actual gameplay"


def key(words):
    return hashlib.sha256(b"".join(w.to_bytes(4,"little") for w in words)).hexdigest()


def parts(words):
    """Split into complete native instructions; never remove a literal alone."""
    pos=0
    out=[]
    for ins in decode(words):
        size=1+(ins.literal is not None)
        out.append(tuple(words[pos:pos+size]))
        pos+=size
    if pos!=len(words)-1 or words[-1]!=END:
        raise ValueError("invalid native program boundary")
    return out


def flatten(blocks):
    return tuple(w for b in blocks for w in b)+(END,)


def features(words):
    """Observable structural diversity proxy, explicitly not source coverage."""
    ops=list(decode(words))
    # Ignore the fixed initialization for the structural features.
    ops=ops[9:]
    seen=set()
    for ins in ops:
        dw,aw,bw=widths(ins.name)
        overlap=bool(dw and ins.dst in (ins.a,ins.b))
        seen.add(("op",ins.name,overlap))
    seen.update(("pair",a.name,b.name) for a,b in zip(ops,ops[1:]))
    return seen


def signature(result):
    kind=result["status"]
    if kind=="refused":
        return (kind,re.sub(r" @ 0x[0-9a-fA-F]+(?=\])","",result["reason"]))
    if kind=="mismatch":
        # Preserve the observed differing outputs, not a claimed root cause.
        return (kind,tuple(i for i,(a,b) in enumerate(zip(result["expected"],result["actual"])) if a!=b))
    return (kind,)


def write_corpus(root, cases):
    rows=[]
    counts=Counter()
    with (root/"programs.txt").open("x") as f:
        for i,words in enumerate(cases):
            ident=f"{i:06d}"
            f.write(ident+" "+" ".join(f"{w:08x}" for w in words)+"\n")
            category="campaign"
            counts[category]+=1
            rows.append({"id":ident,"category":category,"expected":execute(words)})
    (root/"manifest.json").write_text(json.dumps({"schema":1,"seed":0,"profile":"campaign",
        "program_sha256":digest(root/"programs.txt"),"counts":dict(counts),"rows":rows}))


def source_fingerprint(paths):
    return {str(p.resolve()):digest(p) for p in paths}


class Evaluator:
    def __init__(self, bridge,dxc,dxv,root,jobs=2,cache=None,dependencies=()):
        self.tools=(bridge.resolve(),dxc.resolve(),dxv.resolve())
        self.root=root
        self.root.mkdir(parents=True,exist_ok=True)
        self.jobs=jobs
        self.cache=cache
        self.data={}
        self.hits=0
        self.evaluated=0
        self.runs=0
        self.deadline=time.monotonic()+240
        self.source_paths=[Path(__file__),Path(__file__).with_name("check_scalar_oracle.py"),
                           Path(__file__).with_name("generate_scalar_tests.py"),
                           Path(__file__).parent/"reference/rdna2_scalar.py"]
        self.dependencies=list(dependencies)
        # Require an explicit dependency list for persistent reuse. Include
        # current environment (hash only) to avoid mixing different loaders.
        if cache and not dependencies:
            raise ValueError("persistent cache requires explicit compiler library dependencies")
        self.identity={"files":source_fingerprint([*self.tools,*self.source_paths,*dependencies]),
                       "environment_sha256":hashlib.sha256(json.dumps(dict(os.environ),sort_keys=True).encode()).hexdigest(),
                       "python":sys.version}
        # Environment hashes include invocation details; stable callers use
        # the same environment. Never reuse a different compiler fingerprint.
        if cache and cache.exists():
            if cache.stat().st_size>64*1024*1024:
                raise ValueError("cache exceeds 64 MiB")
            saved=json.loads(cache.read_text())
            if saved.get("identity")!=self.identity:
                raise ValueError("cache fingerprint differs; choose a fresh cache")
            self.data=saved["results"]
            if not isinstance(self.data,dict) or len(self.data)>MAX_CASES:
                raise ValueError("invalid cache")
            for k,r in self.data.items():
                if not re.fullmatch(r"[0-9a-f]{64}",k) or not isinstance(r,dict):
                    raise ValueError("invalid cache entry")
                status=r.get("status")
                if status=="pass":
                    if set(r)!={"status"}:
                        raise ValueError("invalid pass receipt")
                elif status=="refused":
                    if not isinstance(r.get("reason"),str):
                        raise ValueError("invalid refusal receipt")
                elif status=="mismatch":
                    if any(not isinstance(r.get(n),list) or len(r[n])!=9 or
                           any(type(v) is not int or not 0<=v<=MASK for v in r[n])
                           for n in ("expected","actual")) or r["expected"]==r["actual"]:
                        raise ValueError("invalid mismatch receipt")
                else:
                    raise ValueError("invalid cached status")

    def evaluate(self,cases, fresh=False):
        if time.monotonic()>self.deadline:
            raise TimeoutError("campaign time budget reached")
        cases=[tuple(w) for w in cases]
        unique=dict((key(w),w) for w in cases)
        missing=[(k,w) for k,w in unique.items() if fresh or k not in self.data]
        self.hits+=len(cases)-len(missing)
        self.evaluated+=len(missing)
        if self.evaluated>MAX_CASES:
            raise ValueError("campaign evaluation budget reached")
        result={}
        if missing:
            self.runs+=1
            with tempfile.TemporaryDirectory(dir=self.root) as tmp:
                d=Path(tmp)
                write_corpus(d,[w for k,w in missing])
                report=check(d,*self.tools,d/"work",self.jobs)
                failures={int(r["id"]):r for r in report["failures"]}
                for i,(k,w) in enumerate(missing):
                    r={a:b for a,b in failures.get(i,{"status":"pass"}).items() if a not in ("id","category")}
                    # Batch-wide tool failures need a separate standalone
                    # replay; never persist or minimize them as case defects.
                    result[k]=r
                    if r["status"] in ("pass","refused","mismatch"):
                        if len(self.data)>=MAX_CASES and k not in self.data:
                            raise ValueError("cache capacity reached")
                        self.data[k]=r
        return [result.get(key(w),self.data.get(key(w))) for w in cases]

    def save(self):
        if source_fingerprint([*self.tools,*self.source_paths,*self.dependencies])!=self.identity["files"]:
            raise ValueError("toolchain changed during campaign")
        if self.cache:
            # One campaign owns one cache. Atomic replace avoids partial JSON.
            with tempfile.NamedTemporaryFile(mode="w",dir=self.cache.parent,delete=False) as stream:
                temp=Path(stream.name)
                json.dump({"identity":self.identity,"results":self.data},stream,sort_keys=True)
            os.replace(temp,self.cache)


def minimize(words,target,evaluate,budget=128):
    """Greedy batched deletion with full oracle re-evaluation each time.

    Keep nine initializer instructions, all observable outputs and exact
    normalized signature. Report local one-deletion minimality, not a global
    minimum or identical root cause. Budget exhaustion is explicit.
    """
    blocks=parts(words)
    prefix,body=blocks[:9],blocks[9:]
    used=0
    granularity=2
    while body and used<budget:
        chunk=max(1,math.ceil(len(body)/granularity))
        options=[body[:i]+body[i+chunk:] for i in range(0,len(body),chunk)]
        options=options[:budget-used]
        candidates=[flatten(prefix+b) for b in options]
        results=evaluate(candidates)
        used+=len(candidates)
        match=next((i for i,r in enumerate(results) if signature(r)==target),None)
        if match is not None:
            body=options[match]
            granularity=max(2,granularity-1)
        elif chunk==1:
            return flatten(prefix+body),used,len(options)==len(body)
        else:
            granularity=min(len(body),granularity*2)
    return flatten(prefix+body),used,not body


def assembly(words):
    lines=[]
    for ins in decode(words):
        dw,aw,bw=widths(ins.name)
        def operand(value,width):
            if value==255:
                return f"0x{ins.literal:08x}"
            if 128<=value<=208:
                return str(value-128 if value<=192 else 192-value)
            return f"s[{value}:{value+1}]" if width==2 else f"s{value}"
        args=([operand(ins.dst,dw)] if dw else [])+[operand(ins.a,aw)]
        if bw:
            args.append(operand(ins.b,bw))
        lines.append(ins.name+" "+", ".join(args))
    return lines+["s_endpgm"]


def llvm_verify(cases,command):
    """Batch independent assembler checks of every distinct native instruction."""
    instructions={}
    for words in cases:
        blocks=parts(words)+[(END,)]
        for text,block in zip(assembly(words),blocks,strict=True):
            old=instructions.setdefault(text,block)
            if old!=block:
                raise ValueError("ambiguous assembly spelling")
    items=list(instructions.items())
    for start in range(0,len(items),8192):
        batch=items[start:start+8192]
        result=subprocess.run([*command,"-triple=amdgcn-amd-amdhsa","-mcpu=gfx1030","-show-encoding"],
                              input="\n".join(text for text,_ in batch)+"\n",text=True,
                              capture_output=True,check=True,timeout=45)
        actual=[bytes(int(v,16) for v in line.split(",")) for line in
                re.findall(r"encoding: \[([^\]]+)\]",result.stdout)]
        expected=[b"".join(w.to_bytes(4,"little") for w in block) for _,block in batch]
        if len(actual)!=len(expected):
            raise ValueError("LLVM did not return every encoding")
        for (text,block),a,b in zip(batch,actual,expected):
            if a!=b:
                raise ValueError(f"LLVM encoding disagreement: {text}: {a.hex()} vs {b.hex()}")
    return len(items)


def canonical_init(values,flag):
    # Use inline integers when LLVM would otherwise shorten a literal.
    words=[]
    for reg,value in enumerate(values,2):
        signed=value if value<2**31 else value-2**32
        if 0<=signed<=64:
            words.append(encode("s_mov_b32",reg,128+signed))
        elif -16<=signed<0:
            words.append(encode("s_mov_b32",reg,192-signed))
        else:
            words.extend((encode("s_mov_b32",reg,255),value))
    words.append(encode("s_cmp_eq_u32",0,128,128 if flag else 129))
    return words


def mutation(seed,count,profile):
    rng=random.Random(seed)
    names=sorted(n for n in FORMS if profile=="extended" or n not in EXTENDED)
    for i in range(count):
        values=[rng.choice(BOUNDARIES) if rng.randrange(3)==0 else rng.getrandbits(32) for _ in range(8)]
        words=canonical_init(values,i%2)
        # Bias toward dependent chains and overlapping register pairs, not
        # arbitrary corrupt machine bytes.
        length=2+i%15
        for j in range(length):
            name=rng.choice(names)
            word,_=operation(name,rng,j%3)
            words.append(word)
        yield tuple(words+[END])


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    for n in ("bridge","dxc","dxv","work","output"):
        parser.add_argument("--"+n,type=Path,required=True)
    parser.add_argument("--cache",type=Path)
    parser.add_argument("--dependency",type=Path,action="append",default=[])
    parser.add_argument("--llvm-mc",nargs="+",required=True,help="LLVM assembler command (including wrapper if needed)")
    parser.add_argument("--seed",type=int,default=0x53434152)
    parser.add_argument("--count",type=int,default=1024)
    parser.add_argument("--profile",choices=("core","extended"),default="core")
    parser.add_argument("--jobs",type=int,choices=(1,2),default=2)
    parser.add_argument("--reduce",type=int,default=8)
    parser.add_argument("--replay-report",type=Path,help="replay reduced synthetic cases from a prior report")
    parser.add_argument("--require-pass",action="store_true",help="also fail on refusals")
    args=parser.parse_args()
    if not 1<=args.count<=8192 or not 0<=args.reduce<=32 or args.output.exists():
        parser.error("count 1..8192, reduce 0..32, fresh output")
    args.work.mkdir(parents=True,exist_ok=False)
    evaluator=Evaluator(args.bridge,args.dxc,args.dxv,args.work,args.jobs,args.cache,args.dependency)
    start=time.monotonic()
    if args.replay_report:
        if args.replay_report.stat().st_size>64*1024*1024:
            parser.error("replay report too large")
        prior=json.loads(args.replay_report.read_text())
        if prior.get("schema")!=1 or prior.get("scope")!=SCOPE:
            parser.error("unsupported replay report")
        cases=[]
        for row in prior["findings"]:
            if row.get("reproduced_twice") is not True:
                continue
            words=tuple(int(w,16) for w in row["reduced_words"])
            if key(words)!=row["reduced_id"] or execute(words)!=row["expected"]:
                parser.error("reproducer fingerprint or expected result differs")
            cases.append(words)
        if not cases or len(cases)>32:
            parser.error("expected 1..32 confirmed reduced cases")
    else:
        cases=list(mutation(args.seed,args.count,args.profile))
    llvm_count=llvm_verify(cases,args.llvm_mc)
    results=evaluator.evaluate(cases,fresh=bool(args.replay_report))
    groups=defaultdict(list)
    features_seen=set()
    representatives=[]
    for w,r in zip(cases,results):
        f=features(w)
        if f-features_seen:
            representatives.append(key(w))
            features_seen.update(f)
        if r["status"]!="pass":
            groups[signature(r)].append((w,r))
    findings=[]
    for sig,group in sorted(groups.items(),key=lambda g:(-len(g[1]),repr(g[0])))[:args.reduce]:
        if sig[0] not in ("refused","mismatch"):
            continue
        original,initial=min(group,key=lambda pair:(len(pair[0]),key(pair[0])))
        # Fresh verification avoids a cached or batch-only failure.
        replay=evaluator.evaluate([original],fresh=True)[0]
        if signature(replay)!=sig:
            findings.append({"signature":sig,"count":len(group),"result":"not_reproduced"})
            continue
        reduced,used,minimal=minimize(original,sig,evaluator.evaluate,budget=min(64,MAX_REDUCTIONS))
        repeats=[evaluator.evaluate([reduced],fresh=True)[0] for _ in range(2)]
        confirmed=all(signature(r)==sig and r==repeats[0] for r in repeats)
        findings.append({"signature":sig,"count":len(group),"original_id":key(original),
                         "original_words":[f"{w:08x}" for w in original],
                         "reduced_id":key(reduced),"reduced_words":[f"{w:08x}" for w in reduced],
                         "expected":execute(reduced),"actual_result":repeats[0],"assembly":assembly(reduced),
                         "reduction_evaluations":used,"one_deletion_minimal":minimal,
                         "reproduced_twice":confirmed})
    # Confirm new findings remain actual LLVM-valid encodings too.
    if findings:
        llvm_verify([tuple(int(w,16) for w in f["reduced_words"]) for f in findings if "reduced_words" in f],args.llvm_mc)
    evaluator.save()
    report={"schema":1,"scope":SCOPE,"seed":args.seed,"profile":args.profile,
            "fingerprint":evaluator.identity,"generated":len(cases),"unique_inputs":len(set(map(key,cases))),
            "outcomes":dict(Counter(r["status"] for r in results)),"llvm_verified_distinct_instructions":llvm_count,
            "signature_counts":[{"signature":sig,"count":len(g)} for sig,g in sorted(groups.items(),key=lambda x:repr(x[0]))],
            "structural_features":len(features_seen),"feature_representatives":representatives,
            "findings":findings,"evaluated":evaluator.evaluated,"cache_or_duplicate_hits":evaluator.hits,
            "compiler_batches":evaluator.runs,"seconds":time.monotonic()-start}
    with args.output.open("x") as stream:
        json.dump(report,stream,indent=2,allow_nan=False)
        stream.write("\n")
    print(json.dumps({k:report[k] for k in ("generated","outcomes","structural_features","evaluated","cache_or_duplicate_hits","seconds")}))
    return int(any(r["status"]!="pass" if args.require_pass else r["status"] in ("mismatch","tool_error","unobservable") for r in results))


if __name__=="__main__":
    raise SystemExit(main())
