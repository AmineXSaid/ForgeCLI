import unittest

from wordcount import count_lines, count_words


class WordCount(unittest.TestCase):
    def test_spaces(self):
        self.assertEqual(count_words("one two  three"), 3)

    def test_newlines_and_tabs(self):
        self.assertEqual(count_words("one\ntwo\tthree\n"), 3)

    def test_empty(self):
        self.assertEqual(count_words(""), 0)
        self.assertEqual(count_lines(""), 0)

    def test_lines(self):
        self.assertEqual(count_lines("a\nb"), 2)
        self.assertEqual(count_lines("a\nb\n"), 2)


if __name__ == "__main__":
    unittest.main()
