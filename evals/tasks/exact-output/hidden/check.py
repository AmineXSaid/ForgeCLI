import os
import subprocess
import sys

want = open(os.path.join(os.environ["KODACT_EVAL_TASK_DIR"], "hidden", "expected.txt")).read()
r = subprocess.run([sys.executable, "stats.py"], capture_output=True, text=True, timeout=60)
if r.returncode != 0:
    sys.exit(f"stats.py failed: {r.stderr[-1500:]}")
# The reader takes stdout as it is: every line must be one of the asked lines, in order.
got = [line for line in r.stdout.splitlines() if line.strip()]
if got != want.splitlines():
    sys.exit(f"stdout:\n{r.stdout[-1500:]}\nwant exactly:\n{want}")
print("ok")
