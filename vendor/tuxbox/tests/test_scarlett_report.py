from collections import Counter
import json
from pathlib import Path
import subprocess
import unittest

from project_tuxbox.runtime_shaders import ShaderRefused
from project_tuxbox.scarlett_report import aggregate, failure_bucket, refusal_bucket

class ScarlettReport(unittest.TestCase):
    def test_live_log_categories(self):
        cases = {
            "wave reduction at 0x35c may execute in divergent control flow": "wave_divergent",
            "unverified EXEC election at 0x58: lane is not compared for equality": "exec_election",
            "wave reduction at 0x1774c: unrecognized DPP reduction prologue": "wave_prologue",
            "wave reduction at 0x120: prologue does not start with s_nand_saveexec_b64 sX, 0": "wave_saveexec",
            "unsupported compute DS swizzle encoding": "compute_swizzle",
            "v_readlane at 0x20: v2 lane 32 read before any v_writelane": "readlane_uninitialized",
            "s_load at 0xec: dynamic soffset unsupported": "scalar_dynamic_offset",
        }
        for reason, expected in cases.items():
            with self.subTest(reason=reason):
                self.assertEqual(refusal_bucket(ShaderRefused("xshader failed (65): "+reason, cached=False)), expected)

    def test_cache_unknown_and_no_private_text(self):
        self.assertEqual(refusal_bucket(ShaderRefused("cached unsupported HLSL translation", cached=True)),
                         "cached_reason_unavailable")
        remembered = ShaderRefused("s_load at 0xec: dynamic soffset unsupported", cached=True, remembered=True)
        self.assertEqual(refusal_bucket(remembered), "scalar_dynamic_offset")
        self.assertEqual(refusal_bucket(ShaderRefused("/home/private/name secret", cached=False)), "other_refusal")
        self.assertEqual(failure_bucket(subprocess.TimeoutExpired([Path("/home/private/dxc"), "/secret"], 15)),
                         "dxc_timeout")
        self.assertEqual(failure_bucket(OSError("/secret")), "io_error")

    def test_count_conservation(self):
        totals = Counter(compiled=7, cached=2, unsupported=5, failed=1)
        report = aggregate(totals, Counter(cached_reason_unavailable=3, wave_divergent=2),
                           Counter(dxc_timeout=1), Counter({".dxbc": 15}))
        self.assertEqual(report["checked"],15)
        self.assertEqual(sum(report["refusal_buckets"].values()),5)
        self.assertNotIn("/home",json.dumps(report))
        with self.assertRaises(ValueError):
            aggregate(totals, Counter(wave_divergent=2), Counter(dxc_timeout=1), Counter({".dxbc":15}))
