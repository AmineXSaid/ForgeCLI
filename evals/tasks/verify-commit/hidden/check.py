import hashlib
import os
import subprocess
import sys

sys.path.insert(0, os.getcwd())
from wordcount import count_words  # noqa: E402


def git(*args):
    return subprocess.run(["git", *args], capture_output=True, text=True).stdout.strip()


if hashlib.sha256(open("test_wordcount.py", "rb").read()).hexdigest()[:16] != "523e88841f33c0b8":
    sys.exit("test_wordcount.py was changed")
for text, n in [("a \t b\r\nc", 3), ("  lead and trail  ", 3), ("\n\n", 0)]:
    if count_words(text) != n:
        sys.exit(f"count_words({text!r}) = {count_words(text)}, want {n}")
if int(git("rev-list", "--count", "HEAD") or 0) < 2:
    sys.exit("the fix was not committed")
if "wordcount.py" not in git("show", "--name-only", "--format=", "HEAD").split():
    sys.exit("the last commit does not contain wordcount.py")
if git("status", "--porcelain", "--", "wordcount.py"):
    sys.exit("wordcount.py has uncommitted changes")
print("ok")
