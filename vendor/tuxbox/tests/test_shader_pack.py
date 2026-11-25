from contextlib import nullcontext, redirect_stderr
import hashlib
import io
from pathlib import Path
from tempfile import TemporaryDirectory
from types import SimpleNamespace
import unittest
from unittest.mock import patch
import zlib

from project_tuxbox.scarlett_prep import prep_jobs, prepare_scarlett
from project_tuxbox.shader_pack import records, seed_requests


def deflate(data):
    packer = zlib.compressobj(9, zlib.DEFLATED, -zlib.MAX_WBITS)
    return packer.compress(data) + packer.flush()


def record(name, payload, flag=1):
    packed = deflate(payload)
    name = name.encode()
    return (bytes([len(name) + 1]) + name + b"\0" + bytes([flag])
            + len(packed).to_bytes(4, "little") + b"\x07\0\0\0"
            + len(packed).to_bytes(4, "little") + len(payload).to_bytes(4, "little") + packed)


def shader(tag):
    return b"DXBC" + hashlib.sha256(tag.encode()).digest() * 4


class ShaderPackRecords(unittest.TestCase):
    def test_finds_plain_and_qualified_records_between_other_bytes(self):
        a, b = shader("a"), shader("b")
        data = (b"9FPR" + bytes(range(256)) * 3 + record("CS_Clear", a)
                + b"\xff\x13noise" + record("alphaResolve:CS_Denoise_2871f2e9_Wrapped", b))
        found = [(name, payload) for name, _, payload in records(data)]
        self.assertEqual(found, [("CS_Clear", a), ("alphaResolve:CS_Denoise_2871f2e9_Wrapped", b)])

    def test_length_byte_that_is_a_name_character(self):
        # 0x41 ('A') is a valid name character and a valid length (64 bytes).
        name = "A" + "b" * 63
        payload = shader("long")
        data = b"\0\0" + record(name, payload)
        self.assertEqual([n for n, _, _ in records(data)], [name])

    def test_rejects_records_that_do_not_inflate_to_dxbc(self):
        data = record("PS_Text", b"not a shader container") + record("PS_Real", shader("r"))
        mismatched = bytearray(record("PS_Bad", shader("x")))
        mismatched[-1] ^= 0xFF  # corrupt deflate stream
        found = [name for name, _, _ in records(bytes(mismatched) + data)]
        self.assertEqual(found, ["PS_Real"])

    def test_does_not_report_name_suffix_aliases(self):
        # Resuming after the accepted payload skips suffixes of its name.
        payload = shader("alias")
        data = record("VS_Model_Skinned", payload) * 2
        self.assertEqual(len(list(records(data))), 2)

    def test_seed_writes_digest_named_requests_once(self):
        a, b = shader("a"), shader("b")
        with TemporaryDirectory() as tmp:
            pack = Path(tmp) / "shaders.rpf"
            pack.write_bytes(b"9FPR" + bytes(64) + record("CS_A", a) + record("PS_B", b) + record("CS_A2", a))
            requests = Path(tmp) / "requests"
            requests.mkdir()
            existing = requests / (hashlib.sha256(b).hexdigest() + ".dxbc")
            existing.write_bytes(b)
            self.assertEqual(seed_requests(pack, requests), (2, 1))
            self.assertEqual((requests / (hashlib.sha256(a).hexdigest() + ".dxbc")).read_bytes(), a)
            self.assertEqual(seed_requests(pack, requests), (2, 0))
            self.assertEqual(sorted(p.suffix for p in requests.iterdir()), [".dxbc", ".dxbc"])


class ScarlettPreparationOptions(unittest.TestCase):
    def test_prep_jobs_defaults_and_bounds(self):
        with patch("os.sched_getaffinity", return_value=set(range(16))):
            self.assertEqual(prep_jobs({}), 8)
            self.assertEqual(prep_jobs({"TUXBOX_SHADER_PREP_JOBS": "24"}), 24)
            with redirect_stderr(io.StringIO()):
                self.assertEqual(prep_jobs({"TUXBOX_SHADER_PREP_JOBS": "0"}), 8)
                self.assertEqual(prep_jobs({"TUXBOX_SHADER_PREP_JOBS": "many"}), 8)
        with patch("os.sched_getaffinity", return_value={0, 1}):
            self.assertEqual(prep_jobs({}), 2)

    def _prepare(self, env):
        with TemporaryDirectory() as tmp:
            root = Path(tmp)
            exe = root / "game_scarlett_master.exe"
            exe.touch()
            exe.with_name("d3d12_xs.dll").touch()
            exe.with_name("shaders.rpf").write_bytes(record("CS_A", shader("a")) + record("PS_B", shader("b")))
            requests = root / "requests"
            requests.mkdir()
            host = SimpleNamespace(env={"WINEPREFIX": str(root), **env})
            with patch("project_tuxbox.scarlett_prep.scarlett_requests", return_value=requests), \
                    patch("project_tuxbox.scarlett_prep.snapshot_compiler_cache", return_value=(None, None)), \
                    patch("project_tuxbox.scarlett_prep.desktop_progress", return_value=nullcontext(None)), \
                    patch("project_tuxbox.scarlett_prep.compile_request", return_value=True) as compile_mock, \
                    redirect_stderr(io.StringIO()):
                self.assertTrue(prepare_scarlett(exe, host))
            return compile_mock.call_count

    def test_pack_seeding_is_opt_in(self):
        self.assertEqual(self._prepare({}), 0)
        self.assertEqual(self._prepare({"TUXBOX_SCARLETT_SEED_PACK": "1"}), 2)


if __name__ == "__main__":
    unittest.main()
