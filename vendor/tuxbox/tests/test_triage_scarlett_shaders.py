from contextlib import redirect_stdout
import io
import json
import os
from pathlib import Path
import sys
from tempfile import TemporaryDirectory
import unittest
import unittest.mock

from tools.triage_scarlett_shaders import command, construct, main, normalize, probe, requests, triage_log

KEY = "ab" * 32


@unittest.skipUnless(sys.platform == "linux", "The fake compiler is a POSIX shell script")
class TriageScarlettShaders(unittest.TestCase):
    def setUp(self):
        temporary = TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.cache = self.root / "requests"
        self.cache.mkdir()

    def test_paired_aggregate_counts_transitions_without_identifiers(self):
        from tools.triage_scarlett_shaders import paired_aggregate
        import hashlib
        sources = [self.cache / (hashlib.sha256(bytes([i])).hexdigest()+".dxbc") for i in range(5)]
        for i, source in enumerate(sources): source.write_bytes(bytes([i]))
        old_tool, new_tool = self.root/"old", self.root/"new"
        old_tool.write_text("old"); new_tool.write_text("new")
        old = [("refused", "scalar_dynamic_offset"), ("refused", "wave_divergent"),
               ("translated", None), ("translated", None), ("failed", "io_error")]
        new = [("translated", None), ("refused", "wave_other"),
               ("refused", "other_refusal"), ("translated", None), ("translated", None)]
        def probe_stub(compiler, source, timeout, *, aggregate_only):
            self.assertTrue(aggregate_only)
            self.assertNotIn(source, sources)
            i = source.read_bytes()[0]
            state, reason = (old if compiler.parent.name == "baseline" else new)[i]
            sources[i].write_bytes(b"changed after snapshot")
            return source, state, reason
        with unittest.mock.patch("tools.triage_scarlett_shaders.probe", side_effect=probe_stub):
            report = paired_aggregate(old_tool, new_tool, sources, 2, 1)
        self.assertEqual((report["recovered"], report["regressed"], report["inconclusive"]), (1,1,1))
        self.assertEqual(report["baseline_refusal_buckets"]["scalar_dynamic_offset"]["translated"], 1)
        self.assertEqual(report["baseline_refusal_buckets"]["wave_divergent"]["refused"], 1)
        self.assertEqual(sum(t["count"] for t in report["transitions"]), 5)
        self.assertNotIn(str(self.root), json.dumps(report))
        self.assertEqual(report["failure_buckets"]["baseline"],{"io_error":1})
        self.assertEqual(report["tool_sha256"]["baseline"],hashlib.sha256(b"old").hexdigest())

    def test_new_fixed_buckets_never_expose_raw_diagnostics(self):
        from project_tuxbox.scarlett_report import refusal_bucket
        from project_tuxbox.runtime_shaders import ShaderRefused
        cases = {
            "s_load at 0x20: code-table read with dynamic soffset unsupported": "scalar_code_table_offset",
            "unsupported mesh launch register s42": "mesh_launch",
            "unverified mesh descriptor at s4 + 0x40": "mesh_root_layout",
            "unsupported mesh output signature element secret": "mesh_signature",
            "unsupported mesh triangle index encoding": "mesh_export",
            "descriptor dataflow did not converge": "descriptor_mapping",
            "LDS exchange with unverified branch convergence": "lds_exchange",
            "compute swizzle inside a loop": "compute_swizzle_other",
            "wave operation requires complete 64-lane compute waves": "wave_width",
            "v4 is used both as a lane spill area and as a vector": "lane_spill",
            "atomic through a heap descriptor unsupported": "heap_atomic",
            "root argument offset outside signature": "packed_root",
            "s_load at 0x20: dynamic soffset unsupported": "scalar_dynamic_offset",
            "unverified mesh shader bindings": "mesh",
            "secret filename": "other_refusal",
        }
        for message, expected in cases.items():
            self.assertEqual(refusal_bucket(ShaderRefused(message, cached=False)), expected)

    def test_masks_counters_and_registers(self):
        self.assertEqual(normalize("warning\ns_load at 0xec: dynamic soffset unsupported\n"),
                         "s_load at 0x_: dynamic soffset unsupported")
        self.assertEqual(normalize("s_buffer_load at 0x108: base s48 is not a buffer descriptor (Some(Table(7)))"),
                         "s_buffer_load at 0x_: base sN is not a buffer descriptor (Some(Table(N)))")
        self.assertEqual(normalize("unmapped descriptor at s9 + 0xe0"), "unmapped descriptor at sN + 0x_")
        self.assertEqual(normalize("MUBUF with a dynamic SOFFSET is unsupported [buffer_load_dwordx3 @ 0x1a0]"),
                         "MUBUF with a dynamic SOFFSET is unsupported")
        self.assertEqual(normalize(""), "(no message)")

    def test_groups_by_named_construct(self):
        self.assertEqual(construct("wave reduction at 0x_: unsupported reduction op v_add_fN"), "wave reduction")
        self.assertEqual(construct("unverified mesh shader bindings"), "unverified mesh shader bindings")
        self.assertEqual(construct("unsupported Hull program: no emitter"), "unsupported Hull program")

    def test_mirrors_runtime_request_flags(self):
        work = Path("/w")
        stencil = self.cache / f"{KEY}_rt3.stencil"
        self.assertEqual(command("x", stencil, work)[1:4], [stencil, "--stencil-target", "3"])
        link = self.cache / "_".join([KEY] * 3)
        link = link.with_name(link.name + ".link")
        self.assertEqual(command("x", link, work)[1:6],
                         [link / "producer.dxbc", "--link-pixel", link / "pixel.dxbc", "--pixel-order", link / "pixel.cso"])
        self.assertIn("--hlsl-out", command("x", self.cache / f"{KEY}.dxbc", work))

    def test_stencil_without_target_is_a_failure_not_a_crash(self):
        source, status, reason = probe("x", self.cache / f"{KEY}.stencil", 1)
        self.assertEqual(status, "failed")
        self.assertIn("render-target suffix", reason)

    def test_rejects_negative_limit(self):
        with redirect_stdout(io.StringIO()), self.assertRaises(SystemExit), \
                unittest.mock.patch("sys.stderr", io.StringIO()):
            main([str(self.cache), "--limit", "-1"])

    def test_lists_only_recorded_requests(self):
        for name in (f"{KEY}.dxbc", f"{KEY}_rt1.stencil", "notes.dxbc", f"{KEY}.cso"):
            (self.cache / name).write_bytes(b"")
        self.assertEqual([p.name for p in requests(self.cache)], [f"{KEY}.dxbc", f"{KEY}_rt1.stencil"])

    def test_log_groups_unique_keys_and_preserves_literal_sampler_states(self):
        a, b, c, d = ("1" * 64, "2" * 64, "3" * 64, "4" * 64)
        def refusal(key, reason):
            return f"[TuxBox 2026-11-24T07:37:00+00:00] Xbox shader {key}: xshader failed (65): {reason}\n"

        wave = "wave reduction at 0x40 may execute in divergent control flow"
        sampler = "mimg at 0x90: s20 not a sampler descriptor (Some(InlineWord(146))) words [0x92, 0xc00000, 0x14500000, 0x0]"
        lines = [
            refusal(a, wave),
            refusal(a, wave.replace("0x40", "0x50")),
            refusal(b, wave),
            refusal(c, sampler),
            refusal(d + "_rt3", sampler.replace("0x14500000", "0x18500000")),
            refusal("invalid-key", wave),
            refusal("5" * 64, wave).replace("failed (65)", "failed (1)"),
            f"Compiled Xbox shader {a}",
        ]
        report = triage_log(lines, 1)
        self.assertEqual(report["totals"], {"refused": 4, "refusal_events": 5})
        self.assertEqual({row["construct"]: row["count"] for row in report["constructs"]},
                         {"mimg": 2, "wave reduction": 2})
        self.assertEqual(len(report["sampler_states"]), 2)
        self.assertEqual(report["sampler_states"][0]["words"],
                         ["0x92", "0xc00000", "0x14500000", "0x0"])
        self.assertTrue(all(len(row["examples"]) == 1 for row in report["reasons"]))

        log = self.root / "scarlett.log"
        log.write_text("".join(lines))
        output = self.root / "report.json"
        with redirect_stdout(io.StringIO()) as stdout:
            self.assertEqual(main(["--log", str(log), "--json", str(output)]), 0)
        self.assertIn("Observed refusals: 4 distinct keys, 5 log events.", stdout.getvalue())
        self.assertIn("Not a coverage estimate", stdout.getvalue())
        self.assertEqual(json.loads(output.read_text())["totals"], report["totals"])

    def test_log_uses_the_last_observed_refusal(self):
        lines = [f"Xbox shader {KEY}: xshader failed (65): first reason",
                 f"Xbox shader {KEY}: xshader failed (65): second reason"]
        report = triage_log(lines, 3)
        self.assertEqual(report["totals"], {"refused": 1, "refusal_events": 2})
        self.assertEqual(report["reasons"][0]["reason"], "second reason")

    def test_ranks_refusals_and_separates_failures(self):
        names = {"1" * 64: 0, "2" * 64: 65, "3" * 64: 65, "4" * 64: 65, "5" * 64: 1}
        for key in names:
            (self.cache / f"{key}.dxbc").write_bytes(key.encode())
        fake = self.root / "xshader"
        fake.write_text(
            "#!/bin/sh\n"
            'case "$(cat "$1")" in\n'
            "  1*) exit 0;;\n"
            "  2*) echo 's_load at 0xec: dynamic soffset unsupported' >&2; exit 65;;\n"
            "  3*) echo 's_load at 0xb0: dynamic soffset unsupported' >&2; exit 65;;\n"
            "  4*) echo 'wave reduction at 0x10 may execute in divergent control flow' >&2; exit 65;;\n"
            "  *) echo 'expected a cooked shader path' >&2; exit 1;;\n"
            "esac\n")
        fake.chmod(0o755)
        output = self.root / "report.json"
        with redirect_stdout(io.StringIO()) as stdout:
            self.assertEqual(main([str(self.cache), "--xshader", str(fake), "--jobs", "2", "--json", str(output)]), 0)
        report = json.loads(output.read_text())
        self.assertEqual(report["totals"], {"requests": 5, "translated": 1, "refused": 3, "failed": 1})
        self.assertEqual(report["reasons"][0]["reason"], "s_load at 0x_: dynamic soffset unsupported")
        self.assertEqual(report["reasons"][0]["count"], 2)
        self.assertEqual(report["constructs"][0], {"construct": "s_load", "count": 2})
        self.assertEqual(report["failures"], [{"reason": "expected a cooked shader path", "count": 1}])
        self.assertIn("| `s_load` | 2 | 66.7% |", stdout.getvalue())
        # The cache is read only: no markers or outputs appear beside the requests.
        self.assertEqual(len(os.listdir(self.cache)), 5)
        shared = self.root / "aggregate.json"
        with redirect_stdout(io.StringIO()):
            self.assertEqual(main([str(self.cache), "--xshader", str(fake), "--jobs", "2",
                                  "--aggregate-only", "--json", str(shared)]), 0)
        report = json.loads(shared.read_text())
        self.assertTrue(report["aggregate_only"])
        self.assertNotIn("xshader", report)
        self.assertEqual(report["totals"], {"requests": 5, "translated": 1, "refused": 3, "failed": 1})
        self.assertEqual({row["reason"]: row["count"] for row in report["reasons"]},
                         {"scalar_dynamic_offset": 2, "wave_divergent": 1})
        self.assertTrue(all(row["examples"] == [] for row in report["reasons"]))
        self.assertEqual(report["failures"], [{"reason": "emitter_failure", "count": 1}])
        self.assertNotIn(str(self.root), shared.read_text())



if __name__ == "__main__":
    unittest.main()
