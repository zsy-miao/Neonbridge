import json
from pathlib import Path, PureWindowsPath
from tempfile import TemporaryDirectory
import unittest
from xml.etree import ElementTree as ET

from project_tuxbox.progress_map import culling_readbacks, library_table, render_map, shader_checks, write_map


class ProgressMapTests(unittest.TestCase):
    def test_active_libraries_match_title_imports_and_observed_loads(self):
        evidence = Path(__file__).resolve().parents[1] / "evidence"
        inventory = json.loads((evidence / "frosty-xbox-library-inventory.json").read_text())
        report = json.loads((evidence / "matrix-awakens/independent-menu.json").read_text())
        status = json.loads((evidence / "library-status.json").read_text())
        self.assertEqual(inventory["title"]["sha256"], report["executable_sha256"])
        direct = {entry["module"].casefold() for entry in inventory["title"]["imports"]}
        loaded = report["loader"]["modules"]
        filenames = {PureWindowsPath(path).name.casefold() for path in loaded}
        binary_hashes = {entry["sha256"] for entry in loaded.values()}
        active = {dll["name"].casefold() for dll in inventory["libraries"]
                  if dll["name"].casefold() in direct | filenames or dll["sha256"] in binary_hashes}
        tracked = {dll["name"].casefold() for dll in status["libraries"]}
        external = {dll["name"].casefold() for dll in status.get("external_libraries", [])}
        self.assertFalse(tracked & external)
        self.assertEqual(active, tracked | external)

    def test_compute_survey_requires_matching_archive_and_valid_counts(self):
        inventory = json.loads((Path(__file__).resolve().parents[1] /
                                "evidence/matrix-awakens/shader-inventory.json").read_text())
        archive = inventory["archives"]["global"]["member_sha256"]
        with TemporaryDirectory() as directory:
            root = Path(directory)
            dest = root / "matrix-awakens"
            dest.mkdir()
            path = dest / "compute-archive-survey.json"
            receipt = {"archive_sha256": archive, "compiler_sha256": "12345678",
                       "stage": {"entries": 2508, "translated": 2254}}
            path.write_text(json.dumps(receipt))
            self.assertTrue(any("2,254/2,508" in note for note in shader_checks(root, inventory)))
            receipt["archive_sha256"] = "different"
            path.write_text(json.dumps(receipt))
            self.assertFalse(any("Global compute archive" in note for note in shader_checks(root, inventory)))
            receipt["archive_sha256"] = archive
            receipt["stage"]["translated"] = 2509
            path.write_text(json.dumps(receipt))
            self.assertFalse(any("Global compute archive" in note for note in shader_checks(root, inventory)))

    def test_culling_readbacks_require_matching_source_and_current_gpu_bytecode(self):
        with TemporaryDirectory() as directory:
            root = Path(directory)
            dest = root / "matrix-awakens"
            dest.mkdir()
            retail = dict(global_archive_sha256="archive", compiler_sha256="old", d3d12_xs_sha256="provider",
                          entry=5, source_sha256="source", dxil_sha256="bytecode", family={"archive_entries": [5, 6]})
            (dest / "instance-cull-retail.json").write_text(json.dumps(retail))
            program = dict(entry=5, source_sha256="source", dxil_sha256="bytecode",
                           cases={"sparse": {"checked_words": 48, "checked_texels": 16}})
            run = dict(name="InstanceCull", compiler_sha256="current", global_archive_sha256="archive",
                       provider_sha256="provider", programs=[program])
            for suffix, adapter in (("amd", "1002:13c0"), ("nvidia", "10de:2705")):
                (dest / f"instance-cull-{suffix}.json").write_text(json.dumps(dict(run, adapter=adapter)))
            self.assertIn("1/2", culling_readbacks(root, "archive", "current", "InstanceCull"))
            self.assertFalse(culling_readbacks(root, "archive", "old", "InstanceCull"))
            self.assertFalse(culling_readbacks(root, "other", "current", "InstanceCull"))
            for changed in (dict(dxil_sha256="different"), dict(cases={})):
                (dest / "instance-cull-amd.json").write_text(json.dumps(
                    dict(run, adapter="1002:13c0", programs=[dict(program, **changed)])))
                self.assertFalse(culling_readbacks(root, "archive", "current", "InstanceCull"))
            # A corrected build may emit new DXIL for the same Xbox program.
            updated = dict(program, dxil_sha256="corrected")
            for suffix, adapter in (("amd", "1002:13c0"), ("nvidia", "10de:2705")):
                (dest / f"instance-cull-{suffix}.json").write_text(json.dumps(
                    dict(run, adapter=adapter, provider_sha256="new provider", programs=[updated])))
            self.assertIn("1/2", culling_readbacks(root, "archive", "current", "InstanceCull"))
            (dest / "instance-cull-nvidia.json").write_text(json.dumps(
                dict(run, adapter="10de:2705", programs=[updated])))
            self.assertFalse(culling_readbacks(root, "archive", "current", "InstanceCull"))

    def test_workspace_map_matches_saved_evidence(self):
        root = Path(__file__).resolve().parents[1]
        self.assertEqual((root / "docs/progress.svg").read_text(encoding="utf-8"),
                         render_map(root / "evidence"),
                         "Regenerate the map with python tuxbox.py status --svg docs/progress.svg")

    def test_projectscarlett_shader_panel_uses_matching_capture_counts(self):
        with TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "projectscarlett-startup.json").write_text(json.dumps({"executable_sha256": "title"}))
            report = root / "projectscarlett-shader-preparation.json"
            run = {"executable_sha256": "title", "compiler_sha256": "compiler", "log_sha256": "log",
                   "requests": {"total": 8, "compiled": 3, "unsupported": 4, "failed": 1}}
            report.write_text(json.dumps(run))
            graphic = render_map(root)
            self.assertIn("3/8 captured requests compiled", graphic)
            self.assertIn("4 unsupported · 1 failed", graphic)
            self.assertIn(">37.5%<", graphic)
            self.assertIn(">Unmeasured<", graphic)
            self.assertNotIn("Furthest run", graphic)
            play = {"executable_sha256": "title", "result": {"exit_code": 5},
                    "title_crash_report": {"sha256": "report", "game_state": "Game Running", "frame": 10146}}
            gameplay = root / "projectscarlett-gameplay.json"
            gameplay.write_text(json.dumps(play))
            self.assertIn("reached gameplay at frame 10,146, then exited with code 5", render_map(root))
            gameplay.write_text(json.dumps(dict(play, executable_sha256="other")))
            self.assertNotIn("Furthest run", render_map(root))
            for changed in (dict(executable_sha256="other"), dict(requests=dict(run["requests"], total=7))):
                report.write_text(json.dumps(dict(run, **changed)))
                self.assertNotIn(">Project Scarlett shader preparation<", render_map(root))

    def write_library_audit(self, root):
        dest = root / "matrix-awakens"
        dest.mkdir()
        (root / "matrix-awakens-launch.json").write_text(json.dumps({"sha256": "game"}))
        (dest / "run.json").write_text("{}")
        ref = f"{root.name}/matrix-awakens/run.json"
        paths = [
            {"name": "Decode frames", "status": "verified", "libraries": ["codec.dll"],
             "provider": "Game package", "scope": "Decoded frames reached the game.", "evidence": [ref]},
            {"name": "Play audio", "status": "partial", "libraries": ["audio.dll", "output.dll"],
             "provider": "Proton", "scope": "Only menu audio checked.", "evidence": [ref]},
        ]
        audit = {"title_sha256": "game", "paths": paths,
                 "optional": [{"name": "Chat", "reason": "No gameplay use observed.", "evidence": [ref]}]}
        path = dest / "library-paths.json"
        path.write_text(json.dumps(audit))
        return path, audit

    def test_library_table_shows_working_provider_and_open_behavior(self):
        with TemporaryDirectory() as directory:
            root = Path(directory)
            self.write_library_audit(root)
            table = library_table(root)
            self.assertIn("1/2 required library behaviors verified", table)
            self.assertIn("Verified · Decode frames · Game package", table)
            self.assertIn("audio.dll, output.dll", table)
            self.assertIn("Only menu audio checked.", table)
            self.assertIn("Chat · No gameplay use observed.", table)
            output = root / "output/map.svg"
            write_map(root, output)
            labels = [node.text for node in ET.parse(output).findall(".//{*}text")]
            self.assertIn("Verified", labels)
            self.assertIn("Partial", labels)
            self.assertNotIn("Chat", labels)

    def test_library_audit_checks_title_evidence_and_unique_behaviors(self):
        with TemporaryDirectory() as directory:
            root = Path(directory)
            path, audit = self.write_library_audit(root)
            path.write_text(json.dumps(dict(audit, title_sha256="other")))
            self.assertIn("No matching", library_table(root))
            self.assertIn("Library behaviors · Unmeasured", render_map(root))
            path.write_text(json.dumps(dict(audit, paths=audit["paths"]*2)))
            with self.assertRaisesRegex(ValueError, "duplicate"):
                library_table(root)
            path.write_text(json.dumps(audit))
            (path.parent / "run.json").unlink()
            with self.assertRaisesRegex(ValueError, "Missing evidence"):
                render_map(root)

    def test_overall_weights_library_behaviors_and_shader_coverage_equally(self):
        with TemporaryDirectory() as directory:
            root = Path(directory)
            self.write_library_audit(root)
            dest = root / "matrix-awakens"
            # Inventorying extra exports does not create a new required behavior.
            (root / "library-status.json").write_text(json.dumps({"libraries": [
                {"name": "codec.dll", "export_coverage": {"total": 1000, "verified": []}}]}))
            archives = {"global": {"member_sha256": "g", "entries": 1},
                        "material": {"member_sha256": "m", "entries": 3}}
            (dest / "shader-inventory.json").write_text(json.dumps({"archives": archives}))
            reports = {}
            for name, count in [("global", 1), ("material", 2)]:
                reports[name] = {"archive_sha256": archives[name]["member_sha256"], "compiler_sha256": "tool",
                                 "stages": {"vertex": {"entries": archives[name]["entries"], "translated": count},
                                            "pixel": {"entries": 0, "translated": 0},
                                            "geometry": {"entries": 0, "translated": 0}}}
            report = dest / "raster-archive-survey.json"
            report.write_text(json.dumps({"archives": reports}))
            graphic = render_map(root)
            self.assertIn(">62.5%<", graphic)
            self.assertIn("Library behaviors · 1/2 verified (50.0%)", graphic)
            self.assertIn("Shader translation · 3/4 entries (75.0%)", graphic)
            reports["material"]["archive_sha256"] = "other"
            report.write_text(json.dumps({"archives": reports}))
            self.assertIn("Shader translation · Unmeasured", render_map(root))
            self.assertNotIn(">62.5%<", render_map(root))

    def test_frosty_milestones_use_matching_retail_profile_and_tuxbox_run(self):
        with TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "matrix-awakens-launch.json").write_text(json.dumps({
                "sha256": "game", "progress_receipt": "latest.json"}))
            receipt = {
                "title_sha256": "game",
                "settings": {"graphics_profile": "packaged", "loose_graphics_overrides_in_package": 0,
                             "ray_tracing": 1, "lumen_hardware_ray_tracing": 1},
                "runs": {"tuxbox": {"log_sha256": "tuxbox", "verified_milestones": ["load_world", "submit_frame"]},
                         "control": {"log_sha256": "other", "verified_milestones": ["use_menu", "play_chase"]}},
                "failure": {"global_shader": 219, "entry": "ExampleCS"},
            }
            file = root / "latest.json"
            file.write_text(json.dumps(receipt))
            svg = ET.fromstring(render_map(root))
            labels = [node.text for node in svg.findall(".//{*}text")]
            self.assertIn("Frosty retail test", labels)
            self.assertNotIn("25%", labels)
            self.assertIn("Packaged settings · World loaded · Frame submitted", labels)
            self.assertNotIn("Menu used", render_map(root))
            self.assertIn("Blocked at global shader 219 · ExampleCS", labels)
            for field, value in [("graphics_profile", "reduced"), ("loose_graphics_overrides_in_package", 1),
                                 ("ray_tracing", 0), ("lumen_hardware_ray_tracing", 0)]:
                with self.subTest(field=field):
                    file.write_text(json.dumps(dict(receipt, settings=dict(receipt["settings"], **{field: value}))))
                    graphic = render_map(root)
                    self.assertIn("No matching retail run", graphic)
                    self.assertNotIn("World loaded", graphic)
                    self.assertNotIn("ExampleCS", graphic)
            file.write_text(json.dumps(dict(receipt, title_sha256="other")))
            self.assertIn("No matching retail run", render_map(root))
            receipt["runs"]["tuxbox"]["verified_milestones"].append("unknown")
            file.write_text(json.dumps(receipt))
            with self.assertRaisesRegex(ValueError, "Unknown Frosty milestone"):
                render_map(root)
