import csv
import json
from pathlib import Path
import struct
import subprocess
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import patch
from xml.etree import ElementTree as ET

from project_tuxbox.progress_map import render_map, shader_table
from project_tuxbox.shader_inventory import ENTRY, MAP, TITLE, inventory, parse_tables, use_frontend_survey
from project_tuxbox.inspect import digest


def counted(records):
    return struct.pack("<I", len(records)) + b"".join(records)


class ShaderInventoryTests(unittest.TestCase):
    def test_archive_counts_memberships_distinct_sources_and_request_evidence(self):
        with TemporaryDirectory() as tmp:
            root = Path(tmp).resolve()
            title = root / "Frosty/Binaries/XSX/Frosty-XSX-Shipping.exe"
            title.parent.mkdir(parents=True)
            title.write_bytes(b"test game")
            pak = root / "Frosty/Content/Paks/Frosty-XSX.pak"
            pak.parent.mkdir(parents=True)
            blob = b"DXBC" + bytes(16) + struct.pack("<4I", 1, 44, 1, 36) + b"DXIL" + bytes(4)
            length = len(blob)
            slots = [ENTRY.pack(0, length, length, 0), ENTRY.pack(length, length, length, 0),
                     ENTRY.pack(2*length, 4, 4, 8)]
            pak.write_bytes(struct.pack("<I", 2) + counted([bytes(20)]) + counted([bytes([i])*20 for i in range(3)]) +
                            counted([MAP.pack(0, 2, 0, 0)]) + counted(slots) + counted([]) +
                            counted([struct.pack("<I", i) for i in (0, 2)]) + blob*2 + b"SPTR")
            raw = root / "shader.dxbc"
            raw.write_bytes(blob)
            source = digest(raw)
            log = root / "game.log"
            absent = "7" * 64
            log.write_text(f"TuxBox d3d12_xs: shader request vertex {source}\n" * 2 +
                           f"TuxBox d3d12_xs: prepared compute shader missing or invalid, source SHA-256 {absent}\n")
            chase = root / "chase.log"
            chase.write_text(f"[TuxBox test] Compiled Xbox shader {source}\n" +
                             f"[TuxBox test] Compiled Xbox shader {absent}_{source}_{source}\n")
            plan = {"executable_sha256": digest(title), "shaders": [{"archive": "global", "entry": 0,
                    "dxbc_sha256": source, "profile": "vs_6_0"}]}
            index = root / "index.csv"
            compiler, independent, dxc, dxv = (root / tool for tool in ("xshader", "strict-xshader", "dxc", "dxv"))
            for tool in (compiler, independent, dxc, dxv):
                tool.write_text(tool.name)

            def run(command):
                if command[0] in (compiler, independent):
                    Path(command[command.index("--hlsl-out") + 1]).write_text("void main() {}")
                    Path(command[command.index("--profile-out") + 1]).write_text("vs_6_0")
                return True

            with patch("project_tuxbox.shader_inventory.TITLE", digest(title)), \
                 patch.dict("project_tuxbox.shader_inventory.ARCHIVES",
                            {"global": (0, pak.stat().st_size, digest(pak)), "material": (0, pak.stat().st_size, digest(pak))},
                            clear=True), patch("project_tuxbox.shader_inventory.decoder"), \
                 patch("project_tuxbox.shader_inventory.survey_command", side_effect=run) as calls, \
                 patch("project_tuxbox.shader_inventory.subprocess.run") as identify:
                identify.return_value = subprocess.CompletedProcess([], 0, b"independent\nlegacy\n")
                with self.assertRaisesRegex(ValueError, "no-default-features"):
                    inventory(root, plan, [], {}, compiler, independent_compiler=compiler)
                identify.return_value = subprocess.CompletedProcess([], 0, b"independent\n")
                result = inventory(root, plan, [], {"chase": chase, "menu": log}, compiler, index=index, dxc=dxc,
                                   independent_compiler=independent)
                self.assertEqual([call.args[0][0] for call in calls.call_args_list], [independent, compiler, dxc, dxv])

                def disagree(command):
                    ok = run(command)
                    if command[0] == compiler:
                        Path(command[command.index("--hlsl-out") + 1]).write_text("different shader")
                    return ok

                calls.side_effect = disagree
                with self.assertRaisesRegex(ValueError, "compilers disagree"):
                    inventory(root, plan, [], {}, compiler, independent_compiler=independent)
            self.assertEqual(result["independent_emitter"], {"compiler_sha256": digest(independent),
                             "programs": [{"source_sha256": source, "stage": "Vertex", "standalone_dxil_validated": True}]})
            self.assertEqual(result["desktop_tools"]["dxv_sha256"], digest(dxv))
            stages = {row["stage"]: row for row in result["stages"]}
            self.assertEqual(stages["Vertex"]["global"], 2)
            self.assertEqual(stages["Vertex"]["material"], 2)
            self.assertEqual(stages["Vertex"]["unique_dxbc"], 1)
            self.assertEqual(stages["Vertex"]["requested_in_profile"], 1)
            self.assertEqual(stages["Compute"]["requested_in_profile"], 0)
            self.assertEqual(stages["Ray hit group"]["unique_dxbc"], 0)
            self.assertEqual(result["traces"]["menu"]["unique_sources"], 2)
            self.assertEqual(result["traces"]["menu"]["trace"], "per-source")
            self.assertEqual(result["traces"]["chase"]["unique_sources"], 1)
            self.assertEqual(result["traces"]["chase"]["trace"], "standalone compilations")
            self.assertEqual(result["requested"][1]["observed_in"], ["chase", "menu"])
            self.assertEqual(result["requested"][0]["archive_entries"], [])
            self.assertIsNone(result["requested"][0]["standalone_dxil_validated"])
            self.assertTrue(result["requested"][1]["standalone_dxil_validated"])
            with index.open() as stream:
                rows = list(csv.DictReader(stream))
            self.assertEqual(len(rows), 6)
            self.assertEqual(rows[0]["shader_maps"], "0")
            self.assertEqual(rows[1]["shader_maps"], "")
            self.assertEqual(rows[1]["standalone_dxil_validated"], "True")
            self.assertEqual(rows[1]["independent_hlsl"], "True")
            self.assertEqual(rows[2]["independent_hlsl"], "")
            evidence = root / "evidence/matrix-awakens"
            evidence.mkdir(parents=True)
            (evidence / "shader-inventory.json").write_text(json.dumps(result))
            svg = ET.fromstring(render_map(evidence.parent))
            self.assertIn("RDNA 2 shader translation", [e.text for e in svg.findall(".//{*}text")])
            self.assertIn(absent, shader_table(evidence.parent))

    def test_mesh_pipeline_counts_require_matching_archive_and_compiler(self):
        with TemporaryDirectory() as directory:
            root = Path(directory)
            evidence = root / "matrix-awakens"
            evidence.mkdir()
            inventory = {
                "archives": {"global": {"member_sha256": "archive"}}, "compiler_sha256": "compiler",
                "requested": [], "linked_variants": 0,
                "stages": [{"stage": "Mesh", "global": 2, "material": 0, "unique_dxbc": 2, "hlsl_emitted": 2,
                            "profiled_dxbc": 0, "requested_in_profile": 0, "requested_dxbc": 0}],
            }
            (evidence / "shader-inventory.json").write_text(json.dumps(inventory))
            receipt = {"global_archive_sha256": "archive", "mesh_pipelines": {
                "compiler_sha256": "compiler", "archive_entries": [18, 19]}}
            path = evidence / "nanite-mesh-nvidia.json"
            path.write_text(json.dumps(receipt))
            self.assertIn("NVIDIA 2/2", shader_table(root))
            path.write_text(json.dumps(dict(receipt, global_archive_sha256="other")))
            self.assertNotIn("NVIDIA 2/2", shader_table(root))
            receipt["mesh_pipelines"]["compiler_sha256"] = "other"
            path.write_text(json.dumps(receipt))
            self.assertNotIn("NVIDIA 2/2", shader_table(root))

    def test_archive_rejects_invalid_tables_and_shader_map_indices(self):
        with self.assertRaisesRegex(ValueError, "exceeds the member"):
            parse_tables(b"\x02\0\0\0\xff\xff\xff\xff")
        tables = [counted([bytes(20)]), counted([]), counted([MAP.pack(0, 1, 0, 0)]), counted([]),
                  counted([]), counted([struct.pack("<I", 99)])]
        with self.assertRaisesRegex(ValueError, "unknown entry"):
            parse_tables(struct.pack("<I", 2) + b"".join(tables))

    def test_survey_reuse_requires_matching_archive_and_stage_identity(self):
        report = {"archives": {"global": {"member_sha256": "a"}}, "executable_sha256": TITLE,
                  "stages": [{"stage": "Vertex", "unique_dxbc": 2}], "compiler_sha256": None}
        previous = dict(report, compiler_sha256="compiler", stages=[{"stage": "Vertex", "unique_dxbc": 2, "hlsl_emitted": 1}])
        use_frontend_survey(report, previous)
        self.assertEqual(report["stages"][0]["hlsl_emitted"], 1)
        self.assertEqual(report["compiler_sha256"], "compiler")
        self.assertNotIn("independent_emitter", report)
        previous["independent_emitter"] = {"compiler_sha256": "strict", "programs": []}
        use_frontend_survey(report, previous)
        self.assertEqual(report["independent_emitter"], previous["independent_emitter"])
        with self.assertRaisesRegex(ValueError, "another build"):
            use_frontend_survey(report, dict(previous, archives={}))
        with self.assertRaisesRegex(ValueError, "source count"):
            use_frontend_survey(report, dict(previous, stages=[]))


if __name__ == "__main__":
    unittest.main()
