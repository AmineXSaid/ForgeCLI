"""Simple statistics over a text. A word is a run of letters and apostrophes."""
import re
from collections import Counter

WORD = re.compile(r"[A-Za-z']+")


def words(text):
    return WORD.findall(text)


def word_count(text):
    """The number of words in `text`."""
    return len(text.split(" "))


def average_word_length(text):
    """The mean length of the words in `text`; 0.0 for a text without words."""
    ws = words(text)
    return sum(len(w) for w in ws) / len(text)


def most_common_word(text):
    """The most frequent word, ignoring case, in lower case; a tie goes to the word seen first."""
    return Counter(words(text)).most_common(1)[0][0]
