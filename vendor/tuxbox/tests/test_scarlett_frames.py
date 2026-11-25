import json
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest

try:
    from PIL import Image, ImageDraw, PngImagePlugin
except ImportError:
    Image = None
if Image is not None:
    from tools.compare_scarlett_frames import compare, main, pairs


@unittest.skipIf(Image is None,"optional Pillow image-analysis dependency")
class FrameTests(unittest.TestCase):
    def setUp(self):
        self.tmp=TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root=Path(self.tmp.name)
        self.a=self.root/"a.png"
        self.b=self.root/"b.png"

    def images(self, mode="RGB", color=(10,20,30)):
        a=Image.new(mode,(128,128),color)
        a.save(self.a)
        return a.copy()

    def test_identity_and_hash(self):
        self.images().save(self.b)
        row=compare(self.a,self.b)
        self.assertTrue(row["passed"])
        self.assertEqual(row["mean_absolute_error"],0)
        self.assertIsNone(row["psnr_db"])
        self.assertIsNone(row["bounding_box"])
        self.assertEqual(row["reference"]["pixel_sha256"],row["candidate"]["pixel_sha256"])

    def test_small_artifact_detected_even_when_global_tolerance_passes(self):
        b=self.images()
        ImageDraw.Draw(b).rectangle((64,64,79,79),fill=(255,20,30))
        b.save(self.b)
        row=compare(self.a,self.b,max_fraction=.02,max_tile_fraction=.01,tile=32,
                    heatmap=self.root/"diff.png")
        self.assertFalse(row["passed"])
        self.assertEqual(row["changed_pixels"],256)
        self.assertEqual(row["bounding_box"],[64,64,80,80])
        self.assertEqual(row["max_tile_fraction"],.25)
        self.assertEqual(row["max_channel_error"],245)
        with Image.open(self.root/"diff.png") as heat:
            self.assertEqual(heat.getpixel((64,64)),245)
            self.assertEqual(heat.getpixel((0,0)),0)

    def test_tolerance_roi_and_partial_tile(self):
        b=self.images()
        b.putpixel((127,127),(11,20,30))
        b.save(self.b)
        row=compare(self.a,self.b,tolerance=1)
        self.assertTrue(row["passed"])
        self.assertEqual(row["changed_pixels"],1)
        self.assertEqual(row["over_tolerance_pixels"],0)
        self.assertTrue(compare(self.a,self.b,roi=(0,0,127,127))["passed"])
        row=compare(self.a,self.b,roi=(120,120,128,128),tile=16)
        self.assertEqual(row["max_tile_fraction"],1/64)
        self.assertEqual(row["bounding_box"],[127,127,128,128])

    def test_alpha_is_not_silently_dropped(self):
        b=self.images("RGBA",(10,20,30,255))
        b.putpixel((0,0),(10,20,30,1))
        b.save(self.b)
        row=compare(self.a,self.b)
        self.assertEqual(row["max_channel_error"],254)
        self.assertEqual(row["changed_pixels"],1)
        self.assertFalse(row["passed"])

    def test_dimensions_color_and_invalid_threshold_fail(self):
        self.images().resize((64,64)).save(self.b)
        with self.assertRaises(ValueError):
            compare(self.a,self.b)
        b=self.images()
        info=PngImagePlugin.PngInfo()
        info.add(b"sRGB",b"\x00")
        b.save(self.b,pnginfo=info)
        with self.assertRaises(ValueError):
            compare(self.a,self.b)
        for fraction in (float("nan"),-1,1.1):
            with self.assertRaises(ValueError):
                compare(self.a,self.b,max_fraction=fraction)

    def test_black_repeated_frames_and_gate(self):
        a,b=self.root/"ref",self.root/"cand"
        a.mkdir();b.mkdir()
        for n,color in enumerate(((100,10,20),(200,30,20),(50,90,120))):
            Image.new("RGB",(32,32),color).save(a/f"{n}.png")
            Image.new("RGB",(32,32),(0,0,0)).save(b/f"{n}.png")
        out=self.root/"report.json"
        self.assertEqual(main([str(a),str(b),"--json",str(out)]),1)
        report=json.loads(out.read_text())
        self.assertEqual(report["candidate_adjacent_identical"],2)
        self.assertEqual(report["reference_adjacent_identical"],0)
        self.assertEqual(report["frames"][0]["candidate"]["near_black_fraction"],1)
        (b/"2.png").unlink()
        with self.assertRaises(ValueError):
            pairs(a,b)

    def test_unsupported_modes_and_animation(self):
        self.images().save(self.b)
        Image.new("L",(128,128),0).save(self.b)
        with self.assertRaises(ValueError):
            compare(self.a,self.b)
        self.images().save(self.b,save_all=True,append_images=[Image.new("RGB",(128,128),(0,0,0))],
                           duration=100,loop=0)
        with self.assertRaises(ValueError):
            compare(self.a,self.b)
