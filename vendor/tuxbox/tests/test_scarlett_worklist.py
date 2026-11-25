import hashlib
import json
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import patch

from project_tuxbox.shader_diagnostics import refusal_detail
from tools.scarlett_worklist import (build_worklist, log_rows, manifest,
                                     paired_rows, selected_sources, main, apply_claims)
from tools.compare_scarlett_shaders import main as compare_main


def key(n):
    return hashlib.sha256(str(n).encode()).hexdigest()


def refused(op="buffer_load_dword"):
    return {"status": "refused", **refusal_detail(
        f"raw buffer IDXEN needs the console descriptor stride [{op} @ 0xabc]")}


def compiled(n=0):
    return {"status": "compiled", "dxil_sha256": key(n)}


class Worklist(unittest.TestCase):
    def test_log_last_refusal_dedup_and_opcode(self):
        rows, events = log_rows([
            f"Xbox shader {key(0)}: xshader failed (65): previous refusal",
            f"Xbox shader {key(0)}: xshader failed (65): raw buffer IDXEN needs the console descriptor stride [buffer_load_dword @ 0x1]",
            f"Xbox shader {key(1)}: xshader failed (65): raw buffer IDXEN needs the console descriptor stride [buffer_load_dwordx2 @ 0x2]",
            f"Xbox shader {key(2)}: xshader failed (1): broken tool",
            f"Xbox shader {key(3)}_rt0: xshader failed (65): unknown",
            f"Xbox shader {key(4)}_{key(5)}_{key(6)}: xshader failed (65): unknown",
        ])
        report = build_worklist(rows, False, events)
        self.assertEqual(report["totals"], {"refused": 4, "refusal_events": 5})
        self.assertIsNone(report["recovered"])
        self.assertEqual(len(report["groups"]), 3)
        self.assertEqual(report["groups"][0]["count"], 2)
        self.assertEqual({r["opcode"] for r in report["groups"]},
                         {None, "buffer_load_dword", "buffer_load_dwordx2"})
        self.assertIn(key(3) + "_rt0.stencil", report["groups"][0]["requests"])

    def test_sampler_variants(self):
        msg = "mimg at 0x3e98: s20 not a sampler descriptor (Some(InlineWord(146))) words [0x92, 0xc00000, 0x14500000, 0x0]"
        a = refusal_detail(msg)
        b = refusal_detail(msg.replace("0x14500000", "0x14000000"))
        rows = [{"request": key(i) + ".dxbc", "after": {"status": "refused", **x}}
                for i, x in enumerate([a, b, a])]
        report = build_worklist(rows, False, 3)
        self.assertEqual(len(report["groups"]), 1)
        self.assertEqual([v["count"] for v in report["groups"][0]["sampler_variants"]], [2, 1])

    def paired_report(self):
        pairs = [(refused(), compiled()), (refused(), refused()),
                 (refused(), refused("buffer_load_dwordx2")),
                 (refused(), {"status": "failed"}), (compiled(), refused()),
                 (compiled(), compiled(1)), ({"status": "failed"}, compiled())]
        rows = [{"request": key(i)+".dxbc", "before": a, "after": b}
                for i, (a, b) in enumerate(pairs)]
        statuses = ("compiled", "refused", "failed")
        return {"requests": len(rows), "rows": rows,
                "transitions": {a: {b: sum(x["before"]["status"] == a and x["after"]["status"] == b
                                           for x in rows) for b in statuses} for a in statuses}}

    def test_paired_recovery_not_new_refusal_or_tool_failure(self):
        report = build_worklist(paired_rows(self.paired_report()), True)
        self.assertEqual(report["recovered"], 1)
        self.assertEqual(report["regressions"], [key(4)+".dxbc"])
        self.assertEqual(report["changed_compiled_outputs"], [key(5)+".dxbc"])
        self.assertEqual(report["inconclusive"], 2)
        first = report["history"][0]
        self.assertEqual([first[k] for k in ("before", "recovered", "still_same",
                          "different_refusal", "inconclusive")], [4, 1, 1, 1, 1])
        self.assertEqual(first["next_blockers"][0]["opcode"], "buffer_load_dwordx2")
        self.assertEqual(sum(g["count"] for g in report["groups"]), 3)
        self.assertEqual(len(manifest(report, [1])["requests"]), 2)
        with self.assertRaises(ValueError):
            manifest(report, [999])

    def test_claims_do_not_change_counts_or_imply_recovery(self):
        report = build_worklist(paired_rows(self.paired_report()), True)
        claims = [
            {"ref": "#48", "opcode": "buffer_load_dword"},
            {"ref": "#58", "reason": refused()["reason"], "opcode": "buffer_load_dword"},
            {"ref": "#56", "opcode": "s_ff0_i32_b32"},
        ]
        apply_claims(report, claims)
        self.assertEqual(report["unclaimed_ranks"], [2])
        self.assertEqual(report["overlapping_claim_ranks"], [1])
        self.assertEqual(report["groups"][0]["claims"], ["#48", "#58"])
        self.assertEqual(report["unmatched_claims"], [claims[2]])
        self.assertEqual(report["recovered"], 1)
        self.assertEqual(sum(g["count"] for g in report["groups"]), 3)
        with self.assertRaises(ValueError):
            apply_claims(report, [{"ref": "#1"}])

    def test_reject_duplicate_or_forged_summary(self):
        for bad in ("duplicate", "summary", "status", "digest"):
            report = self.paired_report()
            if bad == "duplicate":
                report["rows"][1]["request"] = report["rows"][0]["request"]
            elif bad == "summary":
                report["transitions"]["compiled"]["refused"] = 42
            elif bad == "status":
                report["rows"][0]["after"]["status"] = []
            else:
                report["rows"][0]["after"]["dxil_sha256"] = "bad"
            with self.assertRaises(ValueError):
                paired_rows(report)

    def test_selection_and_protected_successes_are_exact(self):
        with TemporaryDirectory() as tmp:
            root = Path(tmp)
            requests = root / "requests"
            requests.mkdir()
            source = root / "paired.json"
            source.write_text(json.dumps(self.paired_report()))
            selection = root / "selected.json"
            self.assertEqual(main(["--comparison", str(source), "--json", str(root/"worklist.json"),
                                   "--manifest", str(selection), "--protect-compiled"]), 0)
            data = json.loads(selection.read_text())
            self.assertEqual(set(data["requests"]), {key(i)+".dxbc" for i in (0,5,6)})
            paths = [requests / n for n in data["requests"]]
            for path in paths:
                path.touch()
            self.assertEqual(selected_sources(paths, data), sorted(paths))
            for bad in ([data["requests"][0]]*2, ["../secret"], [key(9)+".dxbc"]):
                with self.assertRaises(ValueError):
                    selected_sources(paths, {"schema": 1, "requests": bad})
            args = [str(requests), "--select", str(selection), "--json", str(root/"comparison.json")]
            for tool in ("baseline", "candidate", "dxc", "dxv"):
                path = root / tool
                path.touch()
                args += ["--"+tool, str(path)]
            # Even if the baseline also refuses, a previously verified success
            # is protected by the embedded expectation, not a relative gate.
            with patch("tools.compare_scarlett_shaders.compile_one", return_value=refused()):
                self.assertEqual(compare_main(args), 1)
            self.assertEqual(len(json.loads((root/"comparison.json").read_text())["expectation_mismatches"]), 3)


if __name__ == "__main__":
    unittest.main()
