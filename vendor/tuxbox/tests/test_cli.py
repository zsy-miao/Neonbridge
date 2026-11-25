import unittest

import tuxbox


class MisplacedLauncherOptions(unittest.TestCase):
    OPTIONS = {"--mangohud", "--log-file", "--offline"}

    def test_launcher_option_after_the_target_is_reported(self):
        self.assertEqual(tuxbox.misplaced_option(["--mangohud"], self.OPTIONS), "--mangohud")
        self.assertEqual(tuxbox.misplaced_option(["-x", "--log-file=a.log"], self.OPTIONS), "--log-file=a.log")

    def test_game_arguments_pass_through(self):
        self.assertIsNone(tuxbox.misplaced_option([], self.OPTIONS))
        self.assertIsNone(tuxbox.misplaced_option(["--replay", "--report", "r.txt", "-nullrhi"], self.OPTIONS))
