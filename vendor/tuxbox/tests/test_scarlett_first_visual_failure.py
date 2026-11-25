import json
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import patch

try:
    from PIL import Image
    from tools.compare_scarlett_frames import main
except ImportError:
    Image = None

@unittest.skipIf(Image is None, "optional Pillow")
class FirstVisualFailureTests(unittest.TestCase):
    def test_first_failed_frame_is_not_largest_later_difference(self):
        with TemporaryDirectory() as tmp:
            root = Path(tmp)
            a, b = root/"reference", root/"candidate"
            a.mkdir(); b.mkdir()
            for i in range(4):
                ref = Image.new("RGB", (32,32), (80,100,120))
                cand = ref.copy()
                if i == 1: cand.putpixel((5,6), (0,0,0))
                if i == 2: cand = Image.new("RGB", (32,32), (0,0,0))
                ref.save(a/f"{i:02d}.png")
                cand.save(b/f"{i:02d}.png")
            result = root/"result.json"
            self.assertEqual(main([str(a),str(b),"--json",str(result)]),1)
            report = json.loads(result.read_text())
            self.assertEqual(report["first_failed_frame"], {
                "index":1,"frame":"01.png","previous_frame":"00.png","bounding_box":[5,6,6,7]})
            self.assertEqual(report["first_candidate_only_near_black_frame"]["index"],2)
            self.assertEqual(report["first_candidate_only_near_black_frame"]["previous_frame"],"01.png")
            exact = root/"exact.json"
            self.assertEqual(main([str(a),str(a),"--json",str(exact)]),0)
            self.assertIsNone(json.loads(exact.read_text())["first_failed_frame"])
