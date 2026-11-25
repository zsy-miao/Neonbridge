import sys
import unittest
from pathlib import Path
sys.path.insert(0,str(Path(__file__).resolve().parents[1]/"tools"))
from reference.rdna2_packed_integer import result, execute
from check_packed_integer_cpu import generate, assembly, parse_stores

class PackedCPU(unittest.TestCase):
    def test_signed_unsigned_and_wrap(self):
        self.assertEqual(result("v_pk_add_i16",[0x7fff8000,0x0001ffff],0,3,True),0x7fff8000)
        self.assertEqual(result("v_pk_add_u16",[0xffffffff,0x00010001],0,3,True),0xffffffff)
        self.assertEqual(result("v_pk_add_u16",[0xffffffff,0x00010001],0,3),0)
        self.assertEqual(result("v_pk_mad_u16",[0xffffffff]*3,0,7,True),0xffffffff)
        self.assertEqual(result("v_pk_sub_u16",[0,0xffffffff],0,3,True),0)
    def test_single_half_preserves_destination(self):
        initial=[0x12340001,0x43210002,0xabcdef01,0,0,0]
        self.assertEqual(execute(initial,[("v_mul_lo_u16",6,(4,5),0,1,False)])[2],0x0002ef01)
        self.assertEqual(execute(initial,[("v_max_u16",4,(4,5),0,0,False)])[0],0x12340002)
    def test_generator_reproducibility_and_scope(self):
        cases=list(generate(17,1,50))
        self.assertEqual(len(cases),658)
        self.assertEqual(cases,list(generate(17,1,50)))
        self.assertNotEqual(cases,list(generate(18,1,50)))
        self.assertTrue(all(assembly(v,o)[-1]=="s_endpgm" for _,v,o in cases))
    def test_missing_duplicate_and_nonconstant_stores(self):
        line="call void @dx.op.bufferStore.i32(i32 69, %dx.types.Handle %1, i32 0, i32 0, i32 -1, i32 undef, i32 undef, i32 undef, i8 1)"
        self.assertEqual(parse_stores(line,1),[0xffffffff])
        for text,n in [(line,2),(line+"\n"+line,1),(line.replace("i32 -1","i32 %7"),1),("",1)]:
            with self.assertRaises(ValueError):
                parse_stores(text,n)
