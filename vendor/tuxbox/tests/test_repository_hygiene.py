import subprocess
import unittest
from pathlib import Path

from project_tuxbox.verify import status


ROOT = Path(__file__).resolve().parents[1]


class RepositoryHygieneTests(unittest.TestCase):
    def test_results_table_matches_recorded_evidence(self):
        saved = (ROOT / "docs/RESULTS.md").read_text(encoding="utf-8")
        generated = status() + "\n"
        self.assertEqual(saved, generated,
                         "Regenerate with python tuxbox.py status > docs/RESULTS.md")

    def test_no_tracked_files_match_gitignore(self):
        result = subprocess.run(
            ["git", "ls-files", "-c", "--ignored", "--exclude-standard"],
            cwd=ROOT, check=True, capture_output=True, text=True)
        tracked = [line for line in result.stdout.splitlines() if line]
        self.assertEqual(tracked, [],
                         "These tracked files match .gitignore. Remove them from the index.")
