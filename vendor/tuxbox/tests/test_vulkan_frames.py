import json
from pathlib import Path
from tempfile import TemporaryDirectory
import unittest

from tools.profile_vulkan_frames import summarize


class VulkanFramesTests(unittest.TestCase):
    def test_queue_overlap_and_interrupted_trace(self):
        def event(category, name, tick, phase, queue=0):
            return json.dumps({"cat": category, "name": name, "ts": tick, "ph": phase, "tid": queue})
        events = [
            event("Frames", "Frame #4", 1000, "B"),
            event("Command buffers", "first", 1050, "B", 1),
            event("Pipelines", "CS=ABC", 1100, "B", 1),
            event("Pipelines", "CS=ABC", 1400, "E", 1),
            event("Command buffers", "first", 1500, "E", 1),
            event("Command buffers", "second", 1250, "B", 2),
            event("Pipelines", "CS=ABC", 1300, "B", 2),
            event("Pipelines", "CS=ABC", 1700, "E", 2),
            event("Command buffers", "second", 1750, "E", 2),
            event("Frames", "Frame #4", 2000, "E"),
            event("Frames", "Frame #5", 2050, "B"),
            '{"cat":"Command buffers","na',
        ]
        with TemporaryDirectory() as tmp:
            trace = Path(tmp) / "gpu.json"
            trace.write_text('{"traceEvents":[\n' + ",\n".join(events))
            result = summarize(trace, 4, 4)
            self.assertEqual(result["frames"][0]["first_to_last_gpu_timestamp_ms"], 1)
            self.assertEqual(result["frames"][0]["command_buffer_active_ms"], 0.7)
            self.assertEqual(result["pipeline_timings"][0]["mean_ms_per_frame"], 0.7)
            with self.assertRaisesRegex(ValueError, "every completed frame"):
                summarize(trace, 4, 5)
