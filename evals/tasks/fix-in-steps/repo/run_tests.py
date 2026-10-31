"""Checks for textstats.py, in order; stops at the first failure."""
import sys

from textstats import average_word_length, most_common_word, word_count

CHECKS = [
    ("word_count", lambda: word_count("the  cat sat"), 3),
    ("average_word_length", lambda: round(average_word_length("one three five"), 2), 4.0),
    ("most_common_word", lambda: most_common_word("The cat and the hat"), "the"),
]

for name, run, want in CHECKS:
    got = run()
    if got != want:
        print(f"FAIL {name}: expected {want!r}, got {got!r}")
        sys.exit(1)
print(f"all {len(CHECKS)} checks passed")
