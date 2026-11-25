import struct
import unittest
from tools.check_wave_extrema import inputs,expected,fbits

class WaveExtremaTests(unittest.TestCase):
    def test_masked_neutral_and_unsigned_order(self):
        for op,identity in (("umin",0xffffffff),("umax",0),
                            ("fmin",0x7f800000),("fmax",0xff800000)):
            seed,want=expected(64,op,0,[0]*64)
            a,b=map(lambda x:struct.unpack("<384I",x),(seed,want))
            self.assertEqual(a[:224],b[:224])
            self.assertEqual(b[224:288],(identity,)*64)
            self.assertEqual(a[288:],b[288:])
        _,want=expected(32,"umax",3,[0x7fffffff,0x80000000]+[0xffffffff]*30)
        self.assertEqual(struct.unpack("<384I",want)[160:192],(0x80000000,)*32)
    def test_float_negative_and_infinity(self):
        values=[fbits(-3),fbits(-float("inf")),fbits(2)]+[fbits(100)]*61
        _,want=expected(64,"fmin",7,values)
        self.assertEqual(struct.unpack("<384I",want)[224:288],(0xff800000,)*64)
        _,want=expected(64,"fmax",7,values)
        self.assertEqual(struct.unpack("<384I",want)[224:288],(fbits(2),)*64)
    def test_masks_cover_upper_half(self):
        self.assertIn(0xffffffff00000000,[m for _,m,_ in inputs(64,"umin")])
        self.assertEqual(len(list(inputs(64,"fmax"))),16)
