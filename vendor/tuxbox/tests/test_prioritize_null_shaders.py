import json
from pathlib import Path
import unittest
from tools.prioritize_null_shaders import build
from tools.scarlett_worklist import selected_sources

A,B,C,D = (x*64 for x in "abcd")
PREFIX = "TuxBox d3d12_xs: "

def summary(i,total,bound,tracked,compute,raster,overflow=0,overflow_bound=0):
    return PREFIX+f"null usage interval recorded={total} display_bound={bound} tracked={tracked} overflow={overflow} overflow_display_bound={overflow_bound} compute={compute} raster={raster} interval={i} trigger=present_sample"

def row(i,kind,stages,count,bound):
    return PREFIX+f"null usage rank kind={kind} {stages} recorded={count} display_bound={bound} interval={i} trigger=present_sample"

class NullPriorityTests(unittest.TestCase):
    def test_ranking_and_private_manifest(self):
        lines=[row(2,"raster",f"VS={B} PS={C}",3,3),
               summary(1,20,0,1,20,0),row(1,"compute",f"CS={A}",20,0),
               summary(2,3,3,1,0,3)]
        sources=[Path(f"{x}.dxbc") for x in (A,B,C,D)]
        sources += [Path(f"{B}_{C}_{D}.link"),Path(f"{B}_{D}_{C}.link")]
        manifest,aggregate=build(lines,sources,3)
        self.assertEqual(set(manifest["requests"]),{f"{B}.dxbc",f"{C}.dxbc",f"{B}_{C}_{D}.link"})
        self.assertEqual(len(selected_sources(sources,manifest)),3)
        self.assertEqual(aggregate["candidate_requests"],4)
        self.assertEqual(aggregate["recorded"],23)
        for digest in (A,B,C,D): self.assertNotIn(digest,json.dumps(aggregate))

    def test_duplicate_and_inconsistent_intervals_refuse(self):
        line=summary(1,3,1,1,0,3)
        with self.assertRaises(ValueError): build([line,line],[])
        with self.assertRaises(ValueError): build([line,row(1,"raster",f"VS={A}",4,1)],[])
        with self.assertRaises(ValueError): build([row(2,"compute",f"CS={A}",2,0)],[])

    def test_truncated_unknown_and_overflow(self):
        lines=[summary(1,8,1,2,7,1,2,0),row(1,"compute","CS=unknown",2,0),
               row(2,"compute",f"CS={B}",7,0)]
        manifest,aggregate=build(lines,[Path(f"{B}.dxbc")])
        self.assertEqual(manifest["requests"],[])
        self.assertEqual(aggregate["incomplete_intervals"],1)
        self.assertEqual(aggregate["orphan_rank_rows"],1)
        self.assertEqual(aggregate["overflow"],2)
        self.assertEqual(aggregate["unknown_identity_rows"],1)

    def test_rejects_cross_kind_counts_and_invalid_stage_association(self):
        cases=[
            [summary(1,5,0,1,0,5),row(1,"compute",f"CS={A}",5,0)],
            [summary(1,5,0,1,5,0),row(1,"raster",f"VS={A}",5,0)],
            [summary(1,5,0,1,5,0),row(1,"compute",f"VS={A}",5,0)],
            [summary(1,5,0,1,0,5),row(1,"raster",f"CS={A}",5,0)],
            [summary(1,5,1,1,5,0)],
            [summary(1,5,0,1,5,0),row(1,"compute",f"CS={A}",4,0)],
            [summary(1,5,0,1,5,0),row(1,"compute",f"CS={A}",5,0).replace("present_sample","recording_checkpoint")],
            [summary(1,2**64,0,1,2**64,0)],
        ]
        for lines in cases:
            with self.subTest(lines=lines),self.assertRaises(ValueError):
                build(lines,[])

    def test_reports_missing_intervals_and_malformed_records(self):
        lines=[summary(2,5,0,1,5,0),row(2,"compute",f"CS={A}",5,0),
               summary(4,9,2,2,0,9),row(4,"raster",f"VS={B} PS={C}",2,2),
               PREFIX+"null usage rank malformed"]
        manifest,aggregate=build(lines,[Path(f"{B}.dxbc"),Path(f"{A}.dxbc")])
        self.assertEqual(aggregate["missing_intervals_lower_bound"],2)
        self.assertEqual(aggregate["malformed_records"],1)
        self.assertEqual(aggregate["incomplete_intervals"],1)
        self.assertEqual(aggregate["unranked_commands_lower_bound"],7)
        self.assertEqual(manifest["requests"][0],f"{B}.dxbc")
