import hashlib
import io
import json
import os
from pathlib import Path
import struct
import sys
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import patch

from project_tuxbox.shader_prep import SOURCE_CACHE, cooked_shader, prepare_for_launch, prepare_shaders


def record(key, program):
    return {"cache_key": key, "archive": "global", "entry": int(key), "profile": "cs_6_0",
            "observed_dxil_sha256": hashlib.sha256(program).hexdigest()}


@unittest.skipUnless(sys.platform == "linux", "This cache layout belongs to Proton on Linux")
class ShaderPreparation(unittest.TestCase):
    def test_on_demand_rejects_mismatched_source_and_repairs_corrupt_output(self):
        from project_tuxbox.runtime_shaders import compile_request

        with TemporaryDirectory() as tmp:
            root = Path(tmp)
            session = root / "session"
            session.mkdir()
            cache = root / "cache"
            cache.mkdir()
            source = root / "request.dxbc"
            source.write_bytes(b"packaged console shader")
            key = hashlib.sha256(source.read_bytes()).hexdigest()
            tools = ("xshader", "dxc", "dxv")
            bitcode = b"BC\xc0\xde"
            program = struct.pack("<II4sIII", 0x50060, 7, b"DXIL", 0x100, 16, len(bitcode)) + bitcode
            chunk = struct.pack("<4sI", b"DXIL", len(program)) + program
            result = b"DXBC" + bytes(16) + struct.pack("<IIII", 1, 36 + len(chunk), 1, 36) + chunk

            def run(args, *, timeout):
                if args[0] == "xshader":
                    self.assertEqual(args[1].read_bytes(), source.read_bytes())
                    args[args.index("--hlsl-out") + 1].write_text("void main() {}")
                    args[args.index("--profile-out") + 1].write_text("cs_6_0")
                elif args[0] == "dxc":
                    args[args.index("-Fo") + 1].write_bytes(result)
                else:
                    self.assertEqual(args[0], "dxv")
                    self.assertEqual(args[1].read_bytes(), result)

            with patch("project_tuxbox.runtime_shaders.run_tool", side_effect=run) as pipeline:
                with self.assertRaisesRegex(ValueError, "differ"):
                    compile_request(source, "0" * 64, tools, cache, session)
                pipeline.assert_not_called()
                self.assertTrue(compile_request(source, key, tools, cache, session))
                self.assertEqual((session / (key + ".cso")).read_bytes(), result)
                self.assertFalse(compile_request(source, key, tools, cache, session))
                self.assertEqual(pipeline.call_count, 3)
                (cache / (key + ".cso")).write_bytes(result[:-1])
                self.assertTrue(compile_request(source, key, tools, cache, session))
                self.assertEqual(pipeline.call_count, 6)
                self.assertEqual((session / (key + ".cso")).read_bytes(), result)
                (cache / (key + ".sha256")).write_bytes(b"\xff" * 64)
                self.assertTrue(compile_request(source, key, tools, cache, session))
                self.assertEqual(pipeline.call_count, 9)

    def test_source_keys_validate_packaged_bytes_and_reuse_verified_translation(self):
        reference_program = b"measured runtime program"
        first = dict(record("42", reference_program), offset=0, packed_bytes=2, unpacked_bytes=2,
                     dxbc_offset=0, dxbc_bytes=2, dxbc_sha256=hashlib.sha256(b"AA").hexdigest(),
                     hlsl_sha256="identical HLSL", compiled_dxil_sha256="other compiler")
        alias = dict(first, entry=43, offset=2, dxbc_sha256=hashlib.sha256(b"BB").hexdigest())
        plan = {"title": "Fixture", "shaders": [first, alias]}
        with TemporaryDirectory() as tmp:
            pak = Path(tmp) / "game.pak"
            cache = Path(tmp) / SOURCE_CACHE
            cache.mkdir(parents=True)
            (cache / (first["dxbc_sha256"] + ".cso")).write_bytes(reference_program)
            pak.write_bytes(b"AACC")
            with patch("project_tuxbox.shader_prep.compiler_tools", side_effect=AssertionError("compiler called")):
                with self.assertRaisesRegex(ValueError, "Package shader differs"):
                    prepare_shaders(pak, plan, cache, exact_sources=True)
                self.assertFalse((cache / (alias["dxbc_sha256"] + ".cso")).exists())
                # The alias can reuse the first verified output.
                pak.write_bytes(b"AABB")
                self.assertEqual(prepare_shaders(pak, plan, cache, exact_sources=True), (0, 2))
                for r in (first, alias):
                    self.assertEqual((cache / (r["dxbc_sha256"] + ".cso")).read_bytes(), reference_program)

    def test_linked_cache_validates_both_originals_on_warm_launch(self):
        with TemporaryDirectory() as tmp:
            root = Path(tmp)
            pak = root / "game.pak"
            cache = (root / SOURCE_CACHE).with_name("by_gs_ps")
            cache.mkdir(parents=True)
            linked_dxil = b"measured linked geometry shader"
            producer = dict(record("42", linked_dxil), offset=0, packed_bytes=1, unpacked_bytes=1,
                            dxbc_offset=0, dxbc_bytes=1, dxbc_sha256=hashlib.sha256(b"G").hexdigest(),
                            hlsl_sha256="linked HLSL", compiled_dxil_sha256="native digest")
            pixel = dict(producer, entry=43, offset=1, dxbc_sha256=hashlib.sha256(b"P").hexdigest())
            key = producer["dxbc_sha256"] + "_" + pixel["dxbc_sha256"]
            plan = {"title": "Fixture", "shaders": [dict(producer, pixel_input=pixel, pair_key=key)]}
            (cache / (key + ".cso")).write_bytes(linked_dxil)
            pak.write_bytes(b"GP")
            self.assertEqual(prepare_shaders(pak, plan, cache, exact_sources=True, linked=True), (0, 1))
            self.assertEqual((cache / (key + ".cso")).read_bytes(), linked_dxil)
            pak.write_bytes(b"GQ")
            with self.assertRaisesRegex(ValueError, "Package shader differs"):
                prepare_shaders(pak, plan, cache, exact_sources=True, linked=True)

    def test_source_cache_rejects_unverified_but_well_formed_entries(self):
        with TemporaryDirectory() as tmp:
            pak = Path(tmp) / "game.pak"
            pak.write_bytes(b"S")
            cache = Path(tmp) / SOURCE_CACHE
            cache.mkdir(parents=True)
            record_a = dict(record("42", b"measured program"), offset=0, packed_bytes=1, unpacked_bytes=1,
                            dxbc_offset=0, dxbc_bytes=1, dxbc_sha256=hashlib.sha256(b"S").hexdigest(),
                            hlsl_sha256="checked translation", compiled_dxil_sha256="other compiler")
            plan = {"title": "Fixture", "shaders": [record_a]}
            program = struct.pack("<II4sIII4s", 0x50060, 7, b"DXIL", 0x100, 16, 4, b"BC\xc0\xde")
            chunk = struct.pack("<4sI", b"DXIL", len(program)) + program
            unrelated = b"DXBC" + bytes(16) + struct.pack("<IIII", 1, 36 + len(chunk), 1, 36) + chunk
            source_entry = cache / (record_a["dxbc_sha256"] + ".cso")
            source_entry.write_bytes(unrelated)
            with patch("project_tuxbox.shader_prep.compiler_tools", side_effect=ValueError("no compiler")):
                with self.assertRaisesRegex(ValueError, "no compiler"):
                    prepare_shaders(pak, plan, cache, exact_sources=True)
            self.assertFalse(source_entry.exists())

    def test_provider_selects_preparation_and_pipeline_warmup(self):
        from types import SimpleNamespace
        with TemporaryDirectory() as tmp:
            root = Path(tmp)
            exe = root / "fixture.exe"
            exe.write_bytes(b"game")
            provider = root / "d3d12_xs.dll"
            local = root / "local.dll"
            local.write_bytes(b"local")
            compute = dict(record("1", b"compute shader"), dxbc_sha256="compute")
            vertex = dict(record("2", b"vertex shader"), profile="vs_6_0", dxbc_sha256="vertex")
            pixel = dict(record("3", b"pixel shader"), profile="ps_6_1", dxbc_sha256="pixel",
                         compiled_dxil_sha256="native pixel")
            plan = dict(title="Fixture", provider=provider.name, executable_sha256=hashlib.sha256(b"game").hexdigest(),
                        provider_sha256=hashlib.sha256(b"reference").hexdigest(), pipeline_profile="frosty-chase",
                        shader_package="input.pak", shaders=[compute, vertex],
                        source_cache={"compiled_dxil_sha256": {"1": "native compute", "2": "native vertex"},
                                      "aliases": [], "additional_shaders": [pixel]})
            profile = root / "plan.json"
            profile.write_text(json.dumps(plan))
            profile.with_name("frosty-chase-warmup.json").write_text("{}")
            host = SimpleNamespace(backend="proton", env={"WINEPREFIX": str(root)})
            with patch("project_tuxbox.shader_prep.PROFILES", {exe.name: profile}), \
                 patch("project_tuxbox.shader_prep.TUXBOX_PROVIDER", local), \
                 patch("project_tuxbox.shader_prep.prepare_shaders", return_value=(0, 1)) as prepare, \
                 patch("project_tuxbox.chase_warmup.prepare_chase", return_value=(0, 2)) as chase, \
                 patch("project_tuxbox.pipeline_prep.prepare_pipelines") as warm, patch.dict(os.environ):
                os.environ.pop("TUXBOX_SHADER_PREP", None)
                provider.write_bytes(b"unknown")
                self.assertFalse(prepare_for_launch(exe, host))
                prepare.assert_not_called()
                provider.write_bytes(b"local")
                self.assertTrue(prepare_for_launch(exe, host))
                self.assertEqual([r["profile"] for r in prepare.call_args.args[1]["shaders"]],
                                 ["cs_6_0", "vs_6_0", "ps_6_1"])
                self.assertEqual(prepare.call_args.args[2], root / SOURCE_CACHE)
                self.assertIsNone(prepare.call_args.kwargs.get("reference_cache"))
                self.assertTrue(prepare.call_args.kwargs["exact_sources"])
                chase.assert_called_once()
                warm.assert_called_once_with(exe, host, prepare.call_args.args[1], root / SOURCE_CACHE, None,
                                             standalone=True)
                warm.reset_mock()
                chase.reset_mock()
                with patch.dict("os.environ", TUXBOX_SHADER_PREP="menu"):
                    self.assertTrue(prepare_for_launch(exe, host))
                warm.assert_not_called()
                chase.assert_not_called()
                with patch.dict("os.environ", TUXBOX_SHADER_PREP="misspelled"):
                    self.assertTrue(prepare_for_launch(exe, host))
                chase.assert_called_once()
                warm.assert_called_once()
                warm.reset_mock()
                provider.write_bytes(b"reference")
                prepare.reset_mock()
                self.assertFalse(prepare_for_launch(exe, host))
                prepare.assert_not_called()
                warm.assert_not_called()

    def test_missing_compiler_quarantines_broken_files_after_first_miss(self):
        with TemporaryDirectory() as tmp:
            cache = Path(tmp) / "by_hlsl"
            cache.mkdir()
            # Preserve an unfamiliar compiler's complete container. The two
            # other copies have a truncated TOC and a truncated DXIL chunk.
            bitcode = b"BC\xc0\xde"
            program = struct.pack("<II4sIII", 0x50060, 7, b"DXIL", 0x100, 16, len(bitcode)) + bitcode
            chunk = struct.pack("<4sI", b"DXIL", len(program)) + program
            good = b"DXBC" + bytes(16) + struct.pack("<IIII", 1, 36 + len(chunk), 1, 36) + chunk
            plan = {"title": "Fixture", "shaders": [record(str(i), b"other compiler") for i in range(4)]}
            (cache / "1.cso").write_bytes(good)
            bad = {"2": good[:35], "3": good[:-1]}
            for key, data in bad.items():
                (cache / (key + ".cso")).write_bytes(data)

            with patch("project_tuxbox.shader_prep.compiler_tools", side_effect=ValueError("missing compiler")):
                with self.assertRaisesRegex(ValueError, "missing compiler"):
                    prepare_shaders(None, plan, cache)
            self.assertEqual((cache / "1.cso").read_bytes(), good)
            for key, data in bad.items():
                self.assertFalse((cache / (key + ".cso")).exists())
                saved = cache.parent / "tuxbox-invalid" / f"{key}.{hashlib.sha256(data).hexdigest()}.cso"
                self.assertEqual(saved.read_bytes(), data)

    def test_existing_runtime_shader_and_corrupt_cache_recovery(self):
        with TemporaryDirectory() as tmp:
            cache = Path(tmp) / "by_hlsl"
            cache.mkdir()
            a, b = record("1", b"already compiled"), record("2", b"other compiler")
            (cache / "1.cso").write_bytes(b"already compiled")
            (cache / "2.cso").write_bytes(b"truncated")
            plan = {"title": "Fixture", "shaders": [a, b]}

            def compile_one(pak, shader, tools, work):
                self.assertEqual(shader, b)
                result = work / "shader.cso"
                result.write_bytes(b"new verified DXIL")
                return result

            with patch("project_tuxbox.shader_prep.compiler_tools"), \
                 patch("project_tuxbox.shader_prep.compile_shader", side_effect=compile_one) as compile_call:
                self.assertEqual(prepare_shaders(None, plan, cache), (1, 1))
                self.assertEqual(prepare_shaders(None, plan, cache), (0, 2))
                (cache / "2.cso").unlink()
                self.assertEqual(prepare_shaders(None, plan, cache), (1, 1))
                (cache / "2.cso").write_bytes(b"corrupt same size")
                self.assertEqual(prepare_shaders(None, plan, cache), (1, 1))
                # The old receipt belongs to a different provider.
                self.assertEqual(prepare_shaders(None, dict(plan, provider_sha256="new"), cache), (1, 1))
                self.assertEqual(compile_call.call_count, 4)

    def test_interrupted_preparation_keeps_completed_shader(self):
        with TemporaryDirectory() as tmp:
            cache = Path(tmp) / "by_hlsl"
            plan = {"title": "Fixture", "shaders": [record(str(i), b"other compiler") for i in (1, 2)]}
            calls = []
            interrupted = True

            def compile_one(pak, shader, tools, work):
                calls.append(shader["entry"])
                if shader["entry"] == 2 and interrupted:
                    raise ValueError("compiler failed")
                result = work / "shader.cso"
                result.write_bytes(b"new DXIL")
                return result

            with patch("project_tuxbox.shader_prep.compiler_tools"), \
                 patch("project_tuxbox.shader_prep.compile_shader", side_effect=compile_one):
                with self.assertRaisesRegex(ValueError, "compiler failed"):
                    prepare_shaders(None, plan, cache)
                interrupted = False
                self.assertEqual(prepare_shaders(None, plan, cache), (1, 1))
            self.assertEqual(calls, [1, 2, 2])

    def test_rejects_different_cooked_input(self):
        with TemporaryDirectory() as tmp:
            pak = Path(tmp) / "game.pak"
            pak.write_bytes(b"xxAABB")
            shader = dict(entry=5, archive="global", offset=2, packed_bytes=4, unpacked_bytes=4,
                          dxbc_offset=1, dxbc_bytes=2, dxbc_sha256=hashlib.sha256(b"AB").hexdigest())
            self.assertEqual(cooked_shader(pak, shader), b"AB")
            pak.write_bytes(b"xxCDEF")
            with self.assertRaisesRegex(ValueError, "Package shader differs"):
                cooked_shader(pak, shader)

    def test_simultaneous_preparations_share_published_output(self):
        from concurrent.futures import ThreadPoolExecutor
        from threading import Barrier

        with TemporaryDirectory() as tmp:
            cache = Path(tmp) / "by_hlsl"
            plan = {"title": "Fixture", "shaders": [record("1", b"other compiler")]}
            start = Barrier(2)

            def prepare_once():
                start.wait(timeout=5)
                return prepare_shaders(None, plan, cache)

            def compile_one(pak, shader, tools, work):
                result = work / "shader.cso"
                result.write_bytes(b"new DXIL")
                return result

            with patch("project_tuxbox.shader_prep.compiler_tools"), \
                 patch("project_tuxbox.shader_prep.compile_shader", side_effect=compile_one) as compile_call, \
                 ThreadPoolExecutor(max_workers=2) as pool:
                launches = [pool.submit(prepare_once) for _ in range(2)]
                self.assertCountEqual([job.result() for job in launches], [(0, 1), (1, 0)])
                compile_call.assert_called_once()


class ShaderLaunch(unittest.TestCase):
    def run_cli(self, flags, *, prepare=True, game_arguments=(), env=None, exe="game.exe"):
        import os
        from types import SimpleNamespace
        import tuxbox

        with TemporaryDirectory() as tmp, patch("sys.stderr", new_callable=io.StringIO), \
             patch("sys.argv", ["tuxbox", "run", "--log-file", str(Path(tmp) / "run.log"),
                                *flags, exe, *game_arguments]), \
             patch.dict(os.environ, {"TUXBOX_SHADER_PREP": "1", **(env or {})}), \
             patch("tuxbox.launch_target", return_value=Path(exe)), \
             patch("tuxbox.Host") as host, patch("tuxbox.prepare_for_launch") as prepare_fn:
            host.return_value.backend = "native"
            host.return_value.version = "test"
            host.return_value.run_game.return_value = SimpleNamespace(returncode=19)
            if isinstance(prepare, Exception):
                prepare_fn.side_effect = prepare
            else:
                prepare_fn.return_value = prepare
            status = tuxbox.main()
            return status, prepare_fn.call_count, host.return_value.run_game.call_count

    def test_prepare_only_never_starts_game(self):
        self.assertEqual(self.run_cli(["--prepare-only"]), (0, 1, 0))
        self.assertEqual(self.run_cli(["--prepare-only"], prepare=False), (1, 1, 0))
        with self.assertRaisesRegex(ValueError, "compiler failed"):
            self.run_cli(["--prepare-only"], prepare=ValueError("compiler failed"))

    def test_normal_launch_preserves_status_and_bypasses(self):
        self.assertEqual(self.run_cli([]), (19, 1, 1))
        self.assertEqual(self.run_cli([], prepare=ValueError("compiler failed")), (19, 1, 1))
        self.assertEqual(self.run_cli(["--no-shader-prep"]), (19, 0, 1))
        self.assertEqual(self.run_cli([], game_arguments=["-NullRHI"]), (19, 0, 1))
        self.assertEqual(self.run_cli([], env={"TUXBOX_SHADER_PREP": "0"}), (19, 0, 1))

    def test_scarlett_skips_pregame_compilation(self):
        exe = "game_scarlett_master.exe"
        self.assertEqual(self.run_cli([], exe=exe), (19, 0, 1))
        self.assertEqual(self.run_cli(["--prepare-only"], exe=exe), (0, 1, 0))
        self.assertEqual(self.run_cli(["--no-shader-prep"], exe=exe), (19, 0, 1))
