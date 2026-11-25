import hashlib
from pathlib import Path
import struct
import sys
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import patch

from project_tuxbox.chase_warmup import prepare_chase
from project_tuxbox.shader_inventory import ENTRY


def sha(data):
    return hashlib.sha256(data).hexdigest()


def program(stage, tail):
    bitcode = b"BC\xc0\xde" + tail
    header = struct.pack("<II4sIII", stage << 16 | 0x60, (24 + len(bitcode)) // 4, b"DXIL", 0x100, 16, len(bitcode))
    chunk = struct.pack("<4sI", b"DXIL", len(header) + len(bitcode)) + header + bitcode
    return b"DXBC" + bytes(16) + struct.pack("<IIII", 1, 36 + len(chunk), 1, 36) + chunk


def member(rows):
    def counted(records):
        return struct.pack("<I", len(records)) + b"".join(records)
    offset = 0
    descriptors = []
    for stage, data in rows:
        descriptors.append(ENTRY.pack(offset, len(data), len(data), stage))
        offset += len(data)
    return (struct.pack("<I", 2) + counted([]) + counted([bytes(20)] * len(rows)) + counted([]) +
            counted(descriptors) + counted([]) + counted([]) + b"".join(data for _, data in rows))


@unittest.skipUnless(sys.platform == "linux", "The chase cache belongs to the Proton backend")
class ChaseWarmupTests(unittest.TestCase):
    def test_runtime_cache_partial_reuse_and_exact_linked_pixel_order(self):
        source = program(1, b"VERT")
        pixel = program(0, b"PIXL")
        ray_state = struct.pack("<I", 2) + b"RAY0"
        cooked_ray = b"RayGen\0SPTR" + ray_state + bytes(10) + struct.pack("<I", 14)
        # The second entry has no DXBC and must never be decoded.
        global_member = member([(0, source), (0, b"not requested"), (6, cooked_ray)])
        material = member([(3, pixel)])
        archives = {"global": (0, len(global_member), sha(global_member)),
                    "material": (len(global_member), len(material), sha(material))}
        profile = {"title_sha256": "title", "archives": {k: v[2] for k, v in archives.items()},
                   "programs": {"global": [0, 2], "material": [0]}, "links": [["global/0", "material/0"]]}
        with TemporaryDirectory() as tmp:
            root = Path(tmp)
            pak, by_source, cache = root / "game.pak", root / "by_source", root / "runtime"
            pak.write_bytes(global_member + material)
            by_source.mkdir()
            cache.mkdir()
            key, pixel_key = sha(source), sha(pixel)
            tools = ("xshader", "dxc", "dxv")
            pair_orders = []
            stencil_slots = []
            fail_pair = True

            def tool(args, *, timeout):
                if args[0] == "xshader":
                    cooked = args[1].read_bytes()
                    if "--stencil-target" in args:
                        self.assertEqual(cooked, pixel)
                        stencil_slots.append(args[args.index("--stencil-target") + 1])
                    if "--link-pixel" in args:
                        self.assertEqual(args[args.index("--link-pixel") + 1].read_bytes(), pixel)
                        order = args[args.index("--pixel-order") + 1].read_bytes()
                        pair_orders.append(order)
                        if fail_pair:
                            raise ValueError("interrupted pair")
                    args[args.index("--hlsl-out") + 1].write_bytes(cooked[-4:])
                    profile = {source: "vs_6_0", pixel: "ps_6_0", ray_state: "lib_6_3"}[cooked]
                    args[args.index("--profile-out") + 1].write_text(profile)
                elif args[0] == "dxc":
                    profile = args[args.index("-T") + 1]
                    self.assertEqual("-E" in args, profile != "lib_6_3")
                    stage = {"ps_6_0": 0, "vs_6_0": 1, "lib_6_3": 6}[profile]
                    args[args.index("-Fo") + 1].write_bytes(program(stage, args[-1].read_bytes()))
                else:
                    self.assertEqual(args[0], "dxv")

            updates = []

            def prepare():
                def progress(message, completed, total):
                    updates.append((message, completed, total))
                return prepare_chase(pak, by_source, profile, "title", set(), progress)

            with patch.dict("project_tuxbox.chase_warmup.ARCHIVES", archives, clear=True), \
                 patch("project_tuxbox.chase_warmup.decoder"), \
                 patch("project_tuxbox.chase_warmup.snapshot_compiler_cache", return_value=(tools, cache)), \
                 patch("project_tuxbox.chase_warmup.run_tool", side_effect=tool), \
                 patch("project_tuxbox.runtime_shaders.run_tool", side_effect=tool):
                with self.assertRaisesRegex(ValueError, "interrupted pair"):
                    prepare()
                self.assertTrue((cache / (key + ".cso")).exists())
                self.assertEqual((cache / (sha(ray_state) + ".cso")).read_bytes(), program(6, b"RAY0"))
                fail_pair = False
                self.assertEqual(prepare(), (1, 3))
                self.assertTrue(updates)
                updates.clear()
                self.assertEqual(prepare(), (0, 4))
                self.assertIn(("Ray-tracing libraries · 1/1 ready · 0 compiled · 1 reused", 1, 1), updates)
                self.assertIn(("Linked chase shaders · 1/1 ready · 0 compiled · 1 reused", 1, 1), updates)
                # A pre-staged pixel takes precedence, including when an older
                # runtime DXIL and its previously linked producer both exist.
                staged = program(0, b"DIFF")
                (by_source / (pixel_key + ".cso")).write_bytes(staged)
                self.assertEqual(prepare(), (1, 3))
                self.assertEqual(pair_orders, [pixel, pixel, staged])
                self.assertTrue((cache / (f"{key}_{pixel_key}_{sha(staged)}.cso")).is_file())
                profile["stencil_targets"] = [["material/0", 2]]
                self.assertEqual(prepare(), (1, 4))
                self.assertEqual(prepare(), (0, 5))
                self.assertEqual(stencil_slots, ["2"])
                self.assertTrue((cache / f"{pixel_key}_rt2.cso").is_file())
                pak.write_bytes(global_member + material[:-1] + b"!")
                with self.assertRaisesRegex(ValueError, "archive fingerprint"):
                    prepare()
