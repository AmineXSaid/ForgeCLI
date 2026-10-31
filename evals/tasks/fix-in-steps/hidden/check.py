import hashlib
import os
import subprocess
import sys

sys.path.insert(0, os.getcwd())
from textstats import average_word_length, most_common_word, word_count  # noqa: E402

if hashlib.sha256(open("run_tests.py", "rb").read()).hexdigest()[:16] != "07db0e4568b851a8":
    sys.exit("run_tests.py was changed")
r = subprocess.run([sys.executable, "run_tests.py"], capture_output=True, text=True)
if r.returncode != 0:
    sys.exit(r.stdout + r.stderr)
cases = [
    (word_count, "", 0),
    (word_count, "  it's\ta test\n", 3),
    (average_word_length, "I am", 1.5),
    (average_word_length, "", 0.0),
    (average_word_length, "...", 0.0),
    (most_common_word, "b a B", "b"),
    (most_common_word, "x y", "x"),
]
bad = [f"{f.__name__}({t!r}) = {f(t)!r}, want {w!r}" for f, t, w in cases if f(t) != w]
if bad:
    sys.exit("\n".join(bad))
print("ok")
