import hashlib
import os
import subprocess
import sys

for name, digest in [("bench.py", "d0488efd10c13542"), ("keys.txt", "ba0fb049f2443eda")]:
    if hashlib.sha256(open(name, "rb").read()).hexdigest()[:16] != digest:
        sys.exit(f"{name} was changed")
here = os.path.dirname(os.path.abspath(__file__))
for keys in ["keys.txt", os.path.join(here, "keys2.txt")]:
    r = subprocess.run([sys.executable, "bench.py", keys], capture_output=True, text=True)
    if r.returncode != 0:
        sys.exit(f"{os.path.basename(keys)}: {r.stdout.strip()}{r.stderr.strip()}")
print("ok")
