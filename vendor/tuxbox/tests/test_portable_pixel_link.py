import struct
import unittest
from tools.check_portable_pixel_link import expected
from tools.reference.portable_link_fixture import vertex

class PortableLinkTests(unittest.TestCase):
    def test_synthetic_container(self):
        for kind in (1,2,3):
            blob=vertex(kind)
            self.assertEqual(blob[:4],b"DXBC")
            self.assertEqual(struct.unpack_from("<I",blob,24)[0],len(blob))
        self.assertNotEqual(vertex(1),vertex(3))
    def test_color_depth_and_guards(self):
        data=expected()
        self.assertEqual(len(data),8192)
        colors=list(struct.iter_unpack("<4f",data[:4096]))
        self.assertEqual(colors.count((1.,.25,.5,1.)),81)
        self.assertEqual(colors.count((.25,-.5,.75,1.5)),175)
        depth=struct.unpack_from("<16f",data,4096+2*256)
        self.assertEqual(depth[3:12],(.5,)*9)
        self.assertEqual(depth[:3],(.875,)*3)

    def test_runtime_compile_log_keys(self):
        from tools.check_slot86_portable_link import compiled_keys
        self.assertEqual(compiled_keys("Compiled Xbox shader a_b_b\nnot compiled\nCompiled Xbox shader c\n"),["a_b_b","c"])
