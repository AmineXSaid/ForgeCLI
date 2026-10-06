import unittest

from cli import main


class CliTest(unittest.TestCase):
    def test_greets(self):
        self.assertEqual(main(["ada"]), 0)


if __name__ == "__main__":
    unittest.main()
