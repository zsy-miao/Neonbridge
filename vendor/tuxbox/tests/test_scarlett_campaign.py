import json
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest
import sys
from unittest.mock import patch
ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/"tools"))
from scarlett_campaign import (parts,flatten,mutation,features,key,signature,minimize,
                               canonical_init,Evaluator,assembly)
from reference.rdna2_scalar import execute,encode,END


class CampaignTests(unittest.TestCase):
    def test_determinism_and_complete_words(self):
        a=list(mutation(42,50,"core"))
        self.assertEqual(a,list(mutation(42,50,"core")))
        self.assertNotEqual(a,list(mutation(43,50,"core")))
        for words in a:
            self.assertEqual(flatten(parts(words)),words)
            self.assertEqual(len(assembly(words)),len(parts(words))+1)
            self.assertEqual(len(execute(words)),9)
            self.assertTrue(features(words))
        self.assertEqual(len(set(map(key,a))),50)

    def test_minimize_preserves_exact_normalized_signature(self):
        prefix=canonical_init([1]*8,1)
        body=[encode("s_add_u32",2,2,3),encode("s_sub_u32",2,2,3),
              encode("s_ff0_i32_b32",4,2),encode("s_mul_i32",4,4,4)]
        words=tuple(prefix+body+[END])
        def evaluate(cases):
            return [{"status":"refused","reason":"target [s_ff0_i32_b32 @ 0x20]"} if body[2] in case
                    else {"status":"pass"} for case in cases]
        target=signature(evaluate([words])[0])
        reduced,n,minimal=minimize(words,target,evaluate)
        self.assertTrue(minimal)
        self.assertEqual(reduced,tuple(prefix+[body[2],END]))
        self.assertLess(len(reduced),len(words))
        self.assertEqual(signature(evaluate([reduced])[0]),target)
        self.assertEqual(signature({"status":"refused","reason":"target [s_ff0_i32_b32 @ 0x4]"}),target)
        self.assertNotEqual(signature({"status":"refused","reason":"different [s_ff0_i32_b32 @ 0x4]"}),target)

    def test_cache_invalidates_on_dependency_and_never_persists_tool_error(self):
        with TemporaryDirectory() as tmp:
            root=Path(tmp)
            tools=[root/n for n in ("bridge","dxc","dxv","dep")]
            for p in tools:p.write_bytes(b"tool")
            cases=list(mutation(42,2,"core"))
            cache=root/"cache.json"
            report={"failures":[{"id":"000001","category":"campaign","status":"tool_error","reason":"test"}]}
            with patch("scarlett_campaign.check",return_value=report) as check:
                ev=Evaluator(*tools[:3],root,cache=cache,dependencies=tools[3:])
                first=ev.evaluate(cases)
                self.assertEqual(first[0]["status"],"pass")
                self.assertEqual(first[1]["status"],"tool_error")
                self.assertEqual(ev.evaluate([cases[0],cases[0]])[0]["status"],"pass")
                self.assertEqual(check.call_count,1)
                ev.save()
                self.assertEqual(len(json.loads(cache.read_text())["results"]),1)
                ev2=Evaluator(*tools[:3],root,cache=cache,dependencies=tools[3:])
                self.assertEqual(ev2.evaluate([cases[0]])[0]["status"],"pass")
                self.assertEqual(check.call_count,1)
                tools[3].write_bytes(b"changed")
                with self.assertRaises(ValueError):
                    Evaluator(*tools[:3],root,cache=cache,dependencies=tools[3:])
