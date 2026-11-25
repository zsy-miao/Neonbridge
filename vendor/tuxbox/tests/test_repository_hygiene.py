import re
import subprocess
import unittest
from pathlib import Path

from project_tuxbox.verify import status


ROOT = Path(__file__).resolve().parents[1]
RUST_LINE_LIMIT = 1200


def rust_sources():
    """Tracked and new Rust files, by repository path."""
    listed = subprocess.run(["git", "ls-files", "-co", "--exclude-standard", "*.rs"],
                            cwd=ROOT, check=True, capture_output=True, text=True).stdout.splitlines()
    return {name: (ROOT / name).read_text(encoding="utf-8") for name in listed if (ROOT / name).is_file()}


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

    def test_no_rust_file_outgrows_the_limit(self):
        long = {name: len(text.splitlines()) for name, text in rust_sources().items()
                if len(text.splitlines()) > RUST_LINE_LIMIT}
        self.assertEqual(long, {}, "Split these by subject. See docs/SOURCE_LAYOUT.md")

    def test_rust_tests_live_in_the_tests_folder(self):
        inline = re.compile(r"^\s*#\[cfg\(test\)\]\s*\n\s*(?:pub(?:\([^)]*\))? )?mod \w+\s*\{", re.M)
        # The reference controls are single files compiled on their own.
        found = sorted(name for name, text in rust_sources().items()
                       if not name.startswith("tools/reference/") and "/src/tests/" not in name
                       and inline.search(text))
        self.assertEqual(found, [], "Move these into src/tests. See docs/SOURCE_LAYOUT.md")
