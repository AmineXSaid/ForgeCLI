import unittest

from text import slugify


class SlugTest(unittest.TestCase):
    def test_basic(self):
        self.assertEqual(slugify("Hello World"), "hello-world")

    def test_accents(self):
        self.assertEqual(slugify("Crème Brûlée"), "creme-brulee")

    def test_custom_separator(self):
        self.assertEqual(slugify("a b  c", sep="_"), "a_b_c")

    def test_symbols(self):
        self.assertEqual(slugify("C++ & Rust!"), "c-rust")


if __name__ == "__main__":
    unittest.main()
