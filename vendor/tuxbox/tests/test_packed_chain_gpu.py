import random
import struct
import unittest
from tools.check_packed_chain_gpu import seed_and_expected, expected_from_seed, replay_prefixes, include_saturate, GROUPS, STRIDE, START, REGS
from tools.reference.rdna2_packed_integer import execute

class PackedChainGpuTests(unittest.TestCase):
    def test_oracle_preserves_inputs_and_guards(self):
        ops = [("v_pk_add_i16", REGS[0], tuple(REGS[:2]), 0, 3, False)]
        seed,want=seed_and_expected(ops,random.Random(19))
        a,b=struct.unpack("<384I",seed),struct.unpack("<384I",want)
        self.assertEqual(a[:START],b[:START])
        self.assertEqual(a[START+GROUPS*STRIDE:],b[START+GROUPS*STRIDE:])
        for i in range(GROUPS):
            p=START+i*STRIDE
            self.assertEqual(a[p:p+6],b[p:p+6])
            self.assertEqual(execute(a[p:p+6],ops),list(b[p+6:p+12]))


    def test_prefix_replay_preserves_original_inputs(self):
        ops=[("v_pk_add_i16", REGS[0], tuple(REGS[:2]), 0, 3, False),
             ("v_pk_sub_i16", REGS[0], tuple(REGS[:2]), 0, 3, True)]
        seed,want=seed_and_expected(ops,random.Random(7))
        cases=replay_prefixes({"schema":1,"failures":[{"operations":ops,"seed_hex":seed.hex()}]})
        self.assertEqual([len(case[1]) for case in cases],[1,2])
        self.assertEqual(cases[-1][2],seed)
        self.assertEqual(expected_from_seed(seed,cases[-1][1]),want)
        with self.assertRaises(ValueError):
            replay_prefixes({"schema":1,"failures":[{"operations":ops,"seed_hex":"00"}]})

    def test_saturating_pack_boundaries_and_alias(self):
        for name in ("v_sat_pk_u8_i16_e32","v_sat_pk_u8_i16_e64"):
            initial=[0x010000ff,0x80007fff,0xffff0001,0,0,0]
            ops=[(name,4,(4,),0,0,False),(name,5,(5,),0,0,False),
                 (name,6,(6,),0,0,False)]
            self.assertEqual(execute(initial,ops)[:3],[0xffff,0xff,1])
            seed,want=seed_and_expected(ops,random.Random(13))
            cases=replay_prefixes({"schema":1,"failures":[{"operations":ops,"seed_hex":seed.hex()}]})
            self.assertEqual(expected_from_seed(seed,cases[-1][1]),want)
            with self.assertRaises(ValueError):
                execute(initial,[(name,4,(4,),1,0,False)])
    def test_saturating_campaign_keeps_dependency_and_length(self):
        ops=[("v_pk_add_i16",4,(4,5),0,3,False),
             ("v_pk_sub_i16",6,(4,6),0,3,True)]
        cases=include_saturate([("a",ops,None),("b",ops,None)])
        self.assertEqual(len(cases[0][1]),len(ops))
        self.assertEqual(cases[0][1][1],("v_sat_pk_u8_i16_e32",6,(4,),0,0,False))
        self.assertEqual(cases[1][1][1][0],"v_sat_pk_u8_i16_e64")
