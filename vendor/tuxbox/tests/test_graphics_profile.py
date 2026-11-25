import unittest
from unittest.mock import patch

from project_tuxbox.graphics_profile import choose_profile


class GraphicsProfileTests(unittest.TestCase):
    def test_terminal_menu_cancels_on_end_of_input(self):
        with patch("project_tuxbox.graphics_profile.shutil.which", return_value=None), \
                patch("project_tuxbox.graphics_profile.sys.stdin.isatty", return_value=True), \
                patch("builtins.print"), patch("builtins.input", side_effect=EOFError):
            self.assertIsNone(choose_profile({}))
