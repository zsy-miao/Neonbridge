import json
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest
from unittest.mock import patch

from project_tuxbox.d3d12_status import d3d12_table, interface_inventory
from tools.map_d3d12_xs import registered


class D3D12StatusTests(unittest.TestCase):
    def test_compiled_slots_exclude_the_trap_and_keep_duplicate_handlers(self):
        fields = "ptr @_Rfirst, ptr @_R12tuxbox_d3d125proxy16unsupported_slotKj1, ptr @_Rfirst"
        row = f"@_R12tuxbox_d3d126device6DEVICE = internal constant <{{ ptr, ptr, ptr }}> <{{ {fields} }}>, align 8"
        with patch("tools.map_d3d12_xs.TABLES", {"Device": "device::DEVICE"}):
            self.assertEqual(registered([row]), {"Device": [0, 2]})
            with self.assertRaises(ValueError):
                registered([row.replace("ptr @_Rfirst", "ptr null")])
            with self.assertRaises(ValueError):
                registered([row.replace("unsupported_slot", "renamed_handler")])

    def test_catalog_is_per_interface_and_requires_a_handler_for_a_check(self):
        with TemporaryDirectory() as directory:
            evidence = Path(directory)
            (evidence / "matrix-awakens").mkdir()
            path = evidence / "matrix-awakens/d3d12-xs-interfaces.json"
            (evidence / "profile.json").write_text(json.dumps({"package_version": "1", "executable_sha256": "x"}))
            slot = {"slot": 7, "method": "Read", "identified": "profile"}
            data = {"package_version": "1", "title_sha256": "x", "sources": {"profile": "profile.json", "copy": "profile.json"},
                    "registered_slots": {"Device": [7]},
                    "interfaces": [{"name": "Device", "slots": [dict(slot, checks={"profile": "one flag", "copy": "two resources"})]},
                                   {"name": "Queue", "slots": [dict(slot, method="Write")]}]}
            path.write_text(json.dumps(data))
            table = d3d12_table(evidence)
            self.assertIn("Device · 1/1", table)
            self.assertIn("one flag. two resources", table)
            self.assertIn("Queue · 0/1", table)
            checks = data["interfaces"][0]["slots"][0]["checks"]
            checks["unknown receipt"] = "unverifiable"
            path.write_text(json.dumps(data))
            with self.assertRaisesRegex(ValueError, "Invalid D3D12 interface check"):
                interface_inventory(evidence)
            del checks["unknown receipt"]
            data["interfaces"][0]["slots"][0]["slot"] = 8
            path.write_text(json.dumps(data))
            with self.assertRaisesRegex(ValueError, "no registered handler"):
                interface_inventory(evidence)
