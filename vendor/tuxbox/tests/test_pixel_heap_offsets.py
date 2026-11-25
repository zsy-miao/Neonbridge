import struct
import unittest
from tools.check_pixel_heap_offsets import expected

class HeapReferenceTests(unittest.TestCase):
    def test_texture_sampler_and_guard_values(self):
        for tex in (0,1):
            for samp in (0,1):
                data=expected(tex,samp)
                self.assertEqual(len(data),4096)
                pixels=list(struct.iter_unpack("<4f",data))
                color=(0.625,0.25,0.25,1.) if samp else (1.,0.,0.,1.)
                color=tuple(v+(2*tex if i<3 else 0) for i,v in enumerate(color))
                self.assertEqual(pixels[0],(-1.,-2.,-3.,-4.))
                self.assertEqual(pixels[2*16+3],color)
                self.assertEqual(pixels.count(color),81)
                self.assertEqual(pixels.count((-1.,-2.,-3.,-4.)),175)
