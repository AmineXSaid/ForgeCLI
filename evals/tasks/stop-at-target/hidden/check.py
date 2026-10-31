import hashlib
import os
import random
import sys
import tempfile

sys.path.insert(0, os.getcwd())
from check import problems, run  # noqa: E402

if hashlib.sha256(open("check.py", "rb").read()).hexdigest()[:16] != "6e459167b6dcff87":
    sys.exit("check.py was changed")
bins = run("items.txt")
bad = problems("items.txt", bins)
# The same items in another order: a packing tuned to this one file fails.
items = [line.strip() for line in open("items.txt") if line.strip()]
random.Random(3).shuffle(items)
with tempfile.NamedTemporaryFile("w", suffix=".txt", delete=False) as f:
    f.write("\n".join(items) + "\n")
bad += [f"shuffled items: {p}" for p in problems(f.name, run(f.name))]
try:
    written = int(open("bins.txt").read().strip())
except (OSError, ValueError):
    written = None
if written != len(bins):
    bad.append(f"bins.txt says {written}, pack.py uses {len(bins)} bins")
if bad:
    sys.exit("\n".join(bad))
print("ok")
