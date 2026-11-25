import json
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest

from tools.correlate_gpu_journal import correlate, utc_us

START = "2026-11-24T18:00:00+00:00"
END = "2026-11-24T18:01:00+00:00"
BOOT = "a" * 32

class GPUJournalTests(unittest.TestCase):
    def report(self, rows, start=START, end=END):
        with TemporaryDirectory() as tmp:
            p = Path(tmp) / "journal.jsonl"
            p.write_text("\n".join(json.dumps(x) if not isinstance(x, str) else x for x in rows), encoding="utf-8")
            return correlate(p, start, end, BOOT)

    def row(self, message, offset=0, boot=BOOT, transport="kernel"):
        return {"_TRANSPORT": transport, "_BOOT_ID": boot,
                "__REALTIME_TIMESTAMP": str(utc_us(START)+offset), "MESSAGE": message}

    def test_filters_and_sorts(self):
        msg = "NVRM: Xid (PCI:0000:41:00): 31, private-process"
        r = self.report([self.row(msg, 30), self.row(msg, -1),
                         self.row(msg, 1, boot="b"*32),
                         self.row(msg, 1, transport="stdout"),
                         self.row("amdgpu 0000:03:00.0: GPU reset begin!", 10),
                         self.row(msg, 60000001),
                         self.row("unrelated"), "{bad json"])
        self.assertEqual([e["kind"] for e in r["events"]], ["gpu_reset_begin", "nvidia_xid"])
        self.assertEqual(r["events"][1]["code"], 31)
        self.assertEqual(r["counts"]["outside_window"], 2)
        self.assertEqual(r["counts"]["other_or_unknown_boot"], 1)
        self.assertNotIn("private-process", json.dumps(r))

    def test_invalid_fields(self):
        row = self.row("NVRM: Xid (PCI:0000:41:00): 79")
        row["MESSAGE"] = [1, 2]
        r = self.report([row, None])
        self.assertEqual(r["counts"]["invalid_fields"], 1)
        self.assertEqual(r["counts"]["invalid_json"], 1)
        self.assertEqual(r["events"], [])

    def test_inclusive_boundaries(self):
        msg = "NVRM: Xid (PCI:0000:41:00): 79, error"
        r = self.report([self.row(msg, 0), self.row(msg, 60000000)])
        self.assertEqual(len(r["events"]), 2)

    def test_timezone_and_reverse_window(self):
        self.assertEqual(utc_us(START), utc_us("2026-11-24T19:00:00+01:00"))
        with self.assertRaises(ValueError):
            self.report([], start="2026-11-24T18:00:00")
        with self.assertRaises(ValueError):
            self.report([], start=END, end=START)
