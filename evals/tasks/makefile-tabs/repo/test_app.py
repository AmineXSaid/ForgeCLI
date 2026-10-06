import unittest

from app import greet


class AppTest(unittest.TestCase):
    def test_greet(self):
        self.assertEqual(greet("ada"), "hello, ada")


if __name__ == "__main__":
    unittest.main()
