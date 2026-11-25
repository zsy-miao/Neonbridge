import unittest
import struct
from tools.check_dpp8_exchange import reference,cases
class Dpp8Tests(unittest.TestCase):
    def test_broadcast_and_octet_boundary(self):
        sel=(1,0,3,2,5,4,7,6)
        want=struct.unpack("<384I",reference(32,"max",sel,0,0,0))
        self.assertEqual(want[96:128],(1,)*32)
        want=struct.unpack("<384I",reference(64,"or",sel,-1,0,0))
        self.assertEqual(want[96:106],(1,0,3,2,5,4,7,6,9,8))
        self.assertEqual(want[95],0x76540000+95)
        self.assertEqual(want[160],0x76540000+160)
    def test_unsigned_wrap_and_campaign(self):
        want=struct.unpack("<384I",reference(32,"add",(0,)*8,-1,0xffffffff,1))
        self.assertEqual(want[96:104],(0,)*8)
        self.assertEqual(len(list(cases())),192)

    def test_signed_extrema_and_fi_variants(self):
        sel=(1,0,3,2,5,4,7,6)
        for op, want in (("smin",0x80000001),("smax",0x7fffffff),("xor",0xfffffffe)):
            words=struct.unpack("<384I",reference(32,op,sel,0,0x80000000,0x7fffffff))
            self.assertEqual(words[96:128],(want,)*32)
        self.assertEqual({case[-1] for case in cases()},{0,1})
