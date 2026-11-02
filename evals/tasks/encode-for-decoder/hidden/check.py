import hashlib
import os
import random
import subprocess
import sys
import tempfile

sys.path.insert(0, os.getcwd())
if hashlib.sha256(open("decode.py", "rb").read()).hexdigest()[:16] != "5b87ed15b08b04bf":
    sys.exit("decode.py was changed")
from decode import decode  # noqa: E402

text = open("data.txt", "rb").read()
if not os.path.exists("data.fz"):
    sys.exit("data.fz is missing")
packed = open("data.fz", "rb").read()
if decode(packed) != text:
    sys.exit("data.fz does not decode to data.txt")
if len(packed) > 0.40 * len(text):
    sys.exit(f"data.fz is {len(packed)} bytes, more than 40% of data.txt ({len(text)})")

# Inputs encode.py never saw: every byte value, nothing at all, one long run, another log.
rng = random.Random(7)
levels = ["INFO"] * 5 + ["DEBUG", "ERROR"]
log = "".join(
    f"2026-11-03 09:{i // 60:02d}:{i % 60:02d} {rng.choice(levels)} api-{rng.randint(1, 3)} "
    f"req={rng.randint(500, 540)} code={rng.choice([200, 200, 200, 404, 500])} took={rng.randint(1, 250)}ms\n"
    for i in range(300)
).encode()
cases = {
    "random": bytes(rng.randrange(256) for _ in range(4000)),
    "empty": b"",
    "run": b"a" * 1000,
    "log": log,
}
limits = {"run": 0.10, "log": 0.40}
with tempfile.TemporaryDirectory() as tmp:
    for name, data in cases.items():
        src, dst = os.path.join(tmp, name), os.path.join(tmp, name + ".fz")
        open(src, "wb").write(data)
        r = subprocess.run([sys.executable, "encode.py", src, dst], capture_output=True, text=True, timeout=120)
        if r.returncode != 0:
            sys.exit(f"encode.py failed on {name}: {r.stderr[-500:]}")
        out = open(dst, "rb").read()
        try:
            back = decode(out)
        except ValueError as e:
            sys.exit(f"encode.py output for {name} does not decode: {e}")
        if back != data:
            sys.exit(f"encode.py output for {name} decodes to something else")
        if name in limits and len(out) > limits[name] * max(1, len(data)):
            sys.exit(f"encode.py output for {name}: {len(out)} bytes for {len(data)}")
print("ok")
