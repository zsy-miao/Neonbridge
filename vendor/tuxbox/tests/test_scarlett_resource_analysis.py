import json
from itertools import accumulate
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import patch

from tools.scarlett_memory import summarize, compare_reports, snapshot, allocation_queries
from tools.compare_scarlett_performance import check, frame_summary, cpu_summary


def memory_file(path, values, *, end="duration_reached", interval=1, swap=0):
    rows=[{"schema":1,"pid":42,"start_ticks":1,"interval_seconds":interval}]
    for i,v in enumerate(values):
        rows.append({"seconds":i*interval,"rss_kib":v+100,"pss_kib":v,
                     "private_resident_kib":v,"swap_kib":swap,"swap_pss_kib":swap,
                     "pss_plus_swap_pss_kib":v+swap})
    if end:
        rows.append({"end":end})
    path.write_text("".join(json.dumps(row)+"\n" for row in rows))


def frames(path, times, index=0):
    path.write_text("v1\n---SYSTEM INFO---\nos,cpu,gpu\nLinux,CPU,GPU\n"
                    "---FRAME METRICS---\nfps,frametime,elapsed\n"+
                    "".join(f"60,{v},{int(elapsed*1000000)+index}\n" for v,elapsed in zip(times,accumulate(times))))


def manifest(pairs):
    return {"schema":1,"context":{k:"synthetic" for k in
                    ("scenario","hardware","driver","backend","resolution","settings","cache_state")},
            "budgets":{"mean_fps_drop_pct":5,"p95_increase_pct":10,"p99_increase_pct":10,
                       "one_percent_low_drop_pct":10,"stutter_fraction_increase":.01},
            "stutter_ms":50,"min_frames":100,"pairs":pairs}


class MemoryTests(unittest.TestCase):
    def test_growth_peak_and_release(self):
        with TemporaryDirectory() as tmp:
            path=Path(tmp)/"mem.jsonl"
            memory_file(path,[100]*10+[20000]*30)
            r=summarize(path,1000,30)
            self.assertEqual(r["growth"]["result"],"retained_growth_observed")
            self.assertEqual(r["metrics_kib"]["pss_kib"]["peak"],20000)
            memory_file(path,[100]*10+[20000]*20+[100]*10)
            r=summarize(path,1000,30)
            self.assertEqual(r["growth"]["result"],"threshold_not_observed")
            self.assertEqual(r["metrics_kib"]["pss_kib"]["peak"],20000)

    def test_incomplete_and_swap(self):
        with TemporaryDirectory() as tmp:
            path=Path(tmp)/"mem.jsonl"
            memory_file(path,[100]*10+[20000]*30,end=None,swap=123)
            r=summarize(path,1000,30)
            self.assertEqual(r["growth"]["result"],"insufficient_capture")
            self.assertEqual(r["metrics_kib"]["pss_plus_swap_pss_kib"]["peak"],20123)
            with self.assertRaises(ValueError):
                compare_reports(r,r,0)

    def test_peak_regression_and_incomparable_capture(self):
        with TemporaryDirectory() as tmp:
            a,b=Path(tmp)/"a",Path(tmp)/"b"
            memory_file(a,[100]*40);memory_file(b,[200]*40)
            ar,br=summarize(a),summarize(b)
            self.assertFalse(compare_reports(ar,br,99)["passed"])
            self.assertTrue(compare_reports(ar,br,100)["passed"])
            memory_file(b,[200]*40,interval=2)
            with self.assertRaises(ValueError):
                compare_reports(ar,summarize(b),100)

    def test_bad_counters_and_reused_pid(self):
        with TemporaryDirectory() as tmp:
            path=Path(tmp)/"mem"
            memory_file(path,[100]*40)
            text=path.read_text().replace('"pss_plus_swap_pss_kib": 100','"pss_plus_swap_pss_kib": 101',1)
            path.write_text(text)
            with self.assertRaises(ValueError):
                summarize(path)
            with patch("tools.scarlett_memory.birth",return_value=2):
                with self.assertRaises(ProcessLookupError):
                    snapshot(Path(tmp),1)

    def test_interleaved_allocation_queries_and_session_boundaries(self):
        with TemporaryDirectory() as tmp:
            path=Path(tmp)/"log"
            prefix="TuxBox d3d12_xs: "
            item="item0:dim=3 2560x1440x1 mips=1 format=118 samples=1:0 layout=0x0 flags=0x80300006 align=0"
            path.write_text("\n".join([
                "[TuxBox 2026-11-24T00:00:00+00:00] Game log: a",
                prefix+"allocation trace info caller=0x344196 count=1 bytes=18446744073709551615 align=0 id=1",
                prefix+"allocation trace info2-item caller=0x22 count=1 bytes=8192 align=4096 id=2",
                prefix+"allocation trace id=1 "+item,
                prefix+"allocation trace id=2 "+item,
                prefix+"allocation trace id=2 "+item,
                prefix+"allocation trace capped at 256 distinct records",
                "[TuxBox 2026-11-24T00:00:01+00:00] Game log: b",
                prefix+"allocation trace info caller=0x33 count=2 bytes=16384 align=4096 id=1",
                prefix+"allocation trace id=1 "+item,
            ])+"\n")
            r=allocation_queries(path)
            self.assertEqual(r["observed_queries"],3)
            self.assertEqual(r["invalid_size_queries"],1)
            self.assertTrue(r["sessions"][0]["trace_capped"])
            self.assertEqual(r["sessions"][0]["incomplete_records"],0)
            self.assertEqual(r["sessions"][1]["incomplete_records"],1)
            self.assertEqual(r["sessions"][0]["records"][0]["items"][0]["format"],118)



class PerformanceTests(unittest.TestCase):
    def fixture(self,root,regression):
        pairs=[]
        for i in range(3):
            a,b=root/f"a{i}.csv",root/f"b{i}.csv"
            frames(a,[10]*1000,i)
            times=[9]*1000
            if regression and i==1:
                times[-20:]=[100]*20
            frames(b,times,i)
            pairs.append({"baseline":{"frames":a.name,"seconds":20},
                          "candidate":{"frames":b.name,"seconds":20}})
        return manifest(pairs)

    def test_tail_regression_cannot_hide_in_median(self):
        with TemporaryDirectory() as tmp:
            root=Path(tmp)
            m=self.fixture(root,True)
            report=check(m,root)
            self.assertFalse(report["passed"])
            self.assertLess(report["median_changes"]["mean_fps_drop_pct"],0)
            self.assertIn("p99_increase_pct",report["pairs"][1]["violations"])
            self.assertIn("one_percent_low_drop_pct",report["pairs"][1]["violations"])
            self.assertIn("stutter_fraction_increase",report["pairs"][1]["violations"])

    def test_faster_runs_pass(self):
        with TemporaryDirectory() as tmp:
            root=Path(tmp)
            report=check(self.fixture(root,False),root)
            self.assertTrue(report["passed"])
            self.assertAlmostEqual(report["median_changes"]["mean_fps_drop_pct"],-100/9)

    def test_bad_context_duplicate_capture_and_nan(self):
        with TemporaryDirectory() as tmp:
            root=Path(tmp)
            m=self.fixture(root,False)
            m["context"]["driver"]=""
            with self.assertRaises(ValueError):
                check(m,root)
            m["context"]["driver"]="driver"
            m["pairs"][1]["baseline"]=m["pairs"][0]["baseline"]
            with self.assertRaises(ValueError):
                check(m,root)
            m=self.fixture(root,False)
            m["budgets"]["p99_increase_pct"]=float("nan")
            with self.assertRaises(ValueError):
                check(m,root)

    def test_invalid_frame_lifetime_and_bad_rows(self):
        with TemporaryDirectory() as tmp:
            path=Path(tmp)/"frames.csv"
            frames(path,[10]*1000)
            with self.assertRaises(ValueError):
                frame_summary(path,1,50,100,.05)
            path.write_text("fps,frametime\n"+ "60,nan\n"*100)
            with self.assertRaises(ValueError):
                frame_summary(path,20,50,100,.05)

    def test_weighted_cpu_and_pipeline_evidence(self):
        with TemporaryDirectory() as tmp:
            root=Path(tmp)
            m=self.fixture(root,False)
            cpu=root/"cpu.jsonl"
            cpu.write_text("".join(json.dumps({"pid":42,"end_unix_ns":i+1,
                    "seconds":d,"process_one_core_percent":c,"host_busy_percent":20,
                    "clock_ticks_per_second":100,"threads":[{"name":"worker","cpu_ticks":50}]})+"\n"
                    for i,(d,c) in enumerate([(1,100),(3,200)])))
            trace=root/"trace.json"
            trace.write_text(json.dumps({"pid":"pso","ph":"X","name":"0000000000000001 COMP OK","ts":0,"dur":200000})+"\n")
            for side in ("baseline","candidate"):
                m["pairs"][0][side].update(cpu=cpu.name,pipelines=trace.name)
            r=check(m,root)["pairs"][0]["candidate"]
            self.assertEqual(r["cpu"]["process_mean_one_core_percent"],175)
            self.assertEqual(r["pipelines"]["groups"]["compute"]["total_ms"],200)
