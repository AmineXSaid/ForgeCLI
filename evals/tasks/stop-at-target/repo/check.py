"""Check pack.py on items.txt: a valid packing in at most LIMIT bins."""
import subprocess
import sys
from collections import Counter

LIMIT = 33
CAPACITY = 100


def run(path):
    out = subprocess.run([sys.executable, "pack.py", path], capture_output=True, text=True, check=True).stdout
    return [[int(w) for w in line.split()] for line in out.splitlines() if line.strip()]


def problems(path, bins):
    items = Counter(int(line) for line in open(path) if line.strip())
    found = []
    if Counter(w for b in bins for w in b) != items:
        found.append("the bins don't hold exactly the items")
    found += [f"bin {i + 1} holds {sum(b)} > {CAPACITY}" for i, b in enumerate(bins) if sum(b) > CAPACITY]
    if len(bins) > LIMIT:
        found.append(f"{len(bins)} bins, the limit is {LIMIT}")
    return found


if __name__ == "__main__":
    bins = run("items.txt")
    bad = problems("items.txt", bins)
    print(f"{len(bins)} bins (limit {LIMIT}): {'FAIL' if bad else 'PASS'}")
    for p in bad:
        print(" -", p)
    sys.exit(1 if bad else 0)
