"""Tests for native Windows MSVC link order, forwarder libraries, and UTF-8 flags."""
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import build


class RecordingTools:
    def __init__(self):
        self.linker = "link"
        self.links = []
        self.compiles = []

    def rust_library(self, package, *, target_dir=None):
        return Path(f"{package.replace('-', '_')}.lib")

    def compile(self, source, directory, *extra):
        self.compiles.append((Path(source).name, extra))
        return Path(source).stem + ".obj"

    def link(self, output, *inputs, exports=None):
        self.links.append((Path(output).name, list(inputs), Path(exports).name if exports else None))


class BuildMsvcTests(unittest.TestCase):
    def test_xmem_links_xhttp_before_xmem_with_forwarder_lib_and_dllmain(self):
        tools = RecordingTools()
        with tempfile.TemporaryDirectory() as tmp:
            with patch.object(build, "OUTPUT", Path(tmp)):
                build.build_target(tools, "xmem", None)

        dll_links = [(name, inputs, exports) for name, inputs, exports in tools.links if name.endswith(".dll")]
        names = [name for name, _, _ in dll_links]
        self.assertEqual(names, ["tuxbox_xhttp.dll", "xmem.dll"])
        _, xmem_inputs, xmem_def = dll_links[1]
        self.assertEqual(xmem_def, "xmem.def")
        self.assertIn("tuxbox_xhttp.lib", xmem_inputs)
        self.assertIn("/include:DllMain", xmem_inputs)

    def test_d3d12_links_d3dcompiler_for_blob_forwarder(self):
        tools = RecordingTools()
        with tempfile.TemporaryDirectory() as tmp:
            with patch.object(build, "OUTPUT", Path(tmp)), patch.object(build.subprocess, "run") as run:
                build.build_target(tools, "d3d12", "dxc.exe")
                self.assertTrue(run.called)

        d3d_links = [(inputs, exports) for name, inputs, exports in tools.links if name == "d3d12_xs.dll"]
        self.assertEqual(len(d3d_links), 1)
        inputs, exports = d3d_links[0]
        self.assertEqual(exports, "d3d12_xs.def")
        self.assertIn("d3dcompiler.lib", inputs)
        self.assertIn("d3d12.lib", inputs)

    def test_compile_passes_utf8_flag(self):
        tools = build.Toolchain.__new__(build.Toolchain)
        tools.compiler = ["cl"]
        tools.includes = []
        with tempfile.TemporaryDirectory() as tmp:
            with patch.object(build, "require_tool", return_value="cl"), patch.object(build.subprocess, "run") as run:
                tools.compile(Path(tmp) / "sample.cpp", Path(tmp), "/W4", "/WX")
        command = run.call_args[0][0]
        self.assertIn("/utf-8", command)

    def test_def_forwarder_targets_have_matching_import_libs(self):
        # Adversarial check: every DLL forwarder in xmem.def and d3d12_xs.def
        # must have its target import library present on the link line so MSVC
        # link.exe emits a PE forwarder instead of LNK2001.
        forwarder_libs = {
            "tuxbox_xhttp": "tuxbox_xhttp.lib",
            "d3dcompiler_47": "d3dcompiler.lib",
            "d3d12": "d3d12.lib",
        }
        for target, def_rel, dll_name in (
            ("xmem", "runtime/xmem/xmem.def", "xmem.dll"),
            ("d3d12", "runtime/d3d12/d3d12_xs.def", "d3d12_xs.dll"),
        ):
            tools = RecordingTools()
            with tempfile.TemporaryDirectory() as tmp:
                with patch.object(build, "OUTPUT", Path(tmp)), patch.object(build.subprocess, "run"):
                    build.build_target(tools, target, "dxc.exe")
            inputs = next(inp for name, inp, _ in tools.links if name == dll_name)
            for line in (build.ROOT / def_rel).read_text(encoding="utf-8").splitlines()[2:]:
                token = line.split()[0]
                if "=" in token and "." in token.split("=", 1)[1]:
                    target_dll = token.split("=", 1)[1].split(".", 1)[0].lower()
                    self.assertIn(target_dll, forwarder_libs, f"unmapped forwarder DLL {target_dll} in {def_rel}")
                    self.assertIn(forwarder_libs[target_dll], inputs)


if __name__ == "__main__":
    unittest.main()
