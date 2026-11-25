from pathlib import Path
from tempfile import TemporaryDirectory
from types import SimpleNamespace
import json
import sys
import unittest
from unittest.mock import patch

from project_tuxbox import pipeline_prep as prep


class PipelineCache(unittest.TestCase):
    def test_archive_promotion_growth_and_replacement(self):
        with TemporaryDirectory() as temp:
            exe = Path(temp) / "title.exe"
            main, pending = prep.archives(exe, {})
            pending.write_bytes(b"VKS" + b"A" * 77)
            previous = prep.archive_files([main, pending])
            pending.rename(main)
            with main.open("ab") as stream:
                stream.write(b"more pipelines")
            self.assertTrue(prep.still_cached(prep.archive_files([main, pending]), previous))
            main.write_bytes(b"VKS" + b"A" * 50)
            self.assertFalse(prep.still_cached(prep.archive_files([main]), previous))
            main.write_bytes(b"VKS" + b"B" * 100)
            self.assertFalse(prep.still_cached(prep.archive_files([main]), previous))
            self.assertFalse(prep.still_cached(prep.archive_files([main]), [["malformed"]]))
            self.assertEqual(prep.archives(exe, {"VKD3D_SHADER_CACHE_PATH": temp})[0].name,
                             "vkd3d-proton.title.exe.cache")

    @unittest.skipUnless(sys.platform == "linux", "Proton preparation uses Linux file locks")
    def test_driver_cache_reset_and_failed_gpu_run_retry(self):
        with TemporaryDirectory() as temp:
            root = Path(temp)
            cache = root / "by_hlsl"
            cache.mkdir()
            exe = root / "title.exe"
            host = SimpleNamespace(env={"__GL_SHADER_DISK_CACHE_PATH": temp}, version="fixture", command=["wine"])
            records = []
            for archive, index in sorted(prep.COMPUTE_PIPELINES):
                records.append(dict(archive=archive, entry=index, cache_key=str(index)))
                (cache / f"{index}.cso").write_bytes(str(index).encode())
            plan = dict(shaders=records, executable_sha256="title", provider_sha256="provider")
            state = root / "tuxbox-pipelines/pipelines.json"
            driver = root / "GLCache"

            def compile_all(args, **kwargs):
                report = Path(args[3].removeprefix("Z:").replace("\\", "/"))
                report.write_text("".join(f"{r['cache_key']} 2.5\n" for r in records))
                (root / "vkd3d-proton.cache").write_bytes(b"VKS" + b"A" * 100)
                driver.mkdir(exist_ok=True)
                return SimpleNamespace(returncode=0)

            with patch.object(prep, "helper_binary", return_value=exe), \
                 patch.object(prep, "device_key", return_value=["test-gpu"]), \
                 patch.object(prep.subprocess, "run", return_value=SimpleNamespace(returncode=1)) as run:
                with self.assertRaisesRegex(ValueError, "no GPU result"):
                    prep.prepare_pipelines(exe, host, plan, cache)
                self.assertFalse(state.exists())
                run.side_effect = compile_all
                prep.prepare_pipelines(exe, host, plan, cache)
                self.assertEqual(len(json.loads(state.read_text())["pipeline_ms"]), len(records))
                prep.prepare_pipelines(exe, host, plan, cache)
                self.assertEqual(run.call_count, 2)
                driver.rename(root / "old-driver-cache")
                prep.prepare_pipelines(exe, host, plan, cache)
                self.assertEqual(run.call_count, 3)
