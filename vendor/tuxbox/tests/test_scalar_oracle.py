"""Independent reference model and deterministic generation controls."""
import hashlib
import json
from pathlib import Path
import sys
from tempfile import TemporaryDirectory
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/"tools"))
from reference.rdna2_scalar import END, FORMS, MASK, decode, encode, execute
from generate_scalar_tests import initialize, generate
from check_scalar_oracle import stores, load


class ScalarOracle(unittest.TestCase):
    def state(self, words, values=None, flag=0):
        return execute(initialize(values or [0]*8,flag)+words+[END])

    def test_unsigned_carry_and_borrow(self):
        values = [MASK,0,0,0,0,0,0,0]
        self.assertEqual(self.state([0x80028102,0x82038003],values)[:2],[0,1])
        # Subtract 0 - 1, then propagate the borrow through the next word.
        self.assertEqual(self.state([encode("s_sub_u32",2,2,129),
                                    encode("s_subb_u32",3,3,128)])[:2],[MASK,MASK])

    def test_signed_overflow_and_in_place_scc(self):
        self.assertEqual(self.state([encode("s_add_i32",2,2,129)],[0x7fffffff]+[0]*7),
                         [0x80000000]+[0]*7+[1])
        self.assertEqual(self.state([encode("s_sub_i32",2,2,129)],[0x80000000]+[0]*7),
                         [0x7fffffff]+[0]*7+[1])

    def test_pair_shift_reversal_and_preservation(self):
        values = [1,0,0,0,0,0,0,0]
        self.assertEqual(self.state([encode("s_lshl_b64",2,2,160)],values),[0,1]+[0]*6+[1])
        self.assertEqual(self.state([encode("s_brev_b64",2,2)],values,1),[0,0x80000000]+[0]*6+[1])
        self.assertEqual(self.state([encode("s_ff0_i32_b64",2,2)],[MASK,MASK]+[0]*6,1),
                         [MASK,MASK]+[0]*6+[1])

    def test_trace_records_each_instruction_and_condition_code(self):
        trace = []
        words = initialize([MASK]+[0]*7,0)+[encode("s_add_u32",2,2,129),END]
        result = execute(words, trace)
        self.assertEqual(trace[-1]["instruction"],"s_add_u32")
        self.assertEqual(trace[-1]["registers"],result[:-1])
        self.assertEqual(trace[-1]["scc"],1)
        self.assertIsNone(trace[0]["registers"][1])

    def test_unknown_and_uninitialized_fail_closed(self):
        for words in ([END,END],[0xbf820000,END],[0xbe82037e,END],[0xbe8203ff],
                      [0xbe820403,END],[0xbe820302,END],[0xffffffff,END]):
            with self.assertRaises(ValueError):
                execute(words)

    def test_generator_deterministic_distinct_profiles(self):
        a = list(generate(42,2,3,"core"))
        self.assertEqual(a,list(generate(42,2,3,"core")))
        self.assertNotEqual(a,list(generate(43,2,3,"core")))
        b = list(generate(42,2,3,"extended"))
        self.assertEqual(len(b)-len(a),24)
        for _,_,words in a+b:
            self.assertEqual(len(execute(words)),9)
        self.assertEqual(len(FORMS),77)

    def test_store_parser_fails_closed(self):
        line = "call void @dx.op.bufferStore.i32(i32 69, %dx.types.Handle %1, i32 0, i32 0, i32 -1, i32 undef, i32 undef, i32 undef, i8 1)"
        self.assertEqual(stores(line),{0:MASK})
        for bad in (line+"\n"+line,line.replace("i32 -1","i32 %8")):
            with self.assertRaises(ValueError):
                stores(bad)

    def test_corpus_manifest_cannot_fake_reference_values(self):
        with TemporaryDirectory() as tmp:
            root=Path(tmp)
            ident,category,words=next(generate(42,1,0,"core"))
            data=(ident+" "+" ".join(f"{w:08x}" for w in words)+"\n").encode()
            (root/"programs.txt").write_bytes(data)
            manifest={"schema":1,"program_sha256":hashlib.sha256(data).hexdigest(),
                      "rows":[{"id":ident,"category":category,"expected":execute(words)}],
                      "counts":{category:1}}
            path=root/"manifest.json"
            path.write_text(json.dumps(manifest))
            self.assertEqual(load(root),manifest)
            manifest["rows"][0]["expected"][0]^=1
            path.write_text(json.dumps(manifest))
            with self.assertRaises(ValueError):
                load(root)


if __name__=="__main__":
    unittest.main()
