import os
import signal
import subprocess
import sys
import time
import urllib.request

# Runs after the agent has exited: the server must still be there.
pid = None
try:
    pid = int(open("server.pid").read().strip())
except (OSError, ValueError):
    pass
try:
    try:
        port = int(open("port.txt").read().strip())
    except (OSError, ValueError):
        sys.exit("port.txt is missing or not a number")
    want = {name: open(os.path.join("site", name), "rb").read() for name in ("index.html", "notes.txt")}
    for name, body in want.items():
        for attempt in range(3):
            try:
                got = urllib.request.urlopen(f"http://127.0.0.1:{port}/{name}", timeout=5).read()
                break
            except OSError as e:
                if attempt == 2:
                    sys.exit(f"nothing answers on port {port} after the run: {e}")
                time.sleep(1)
        if got != body:
            sys.exit(f"{name} served from port {port} differs from site/{name}")
    if pid is None:
        sys.exit("server.pid is missing or not a number")
    print("ok")
finally:
    # Don't leave the server behind on the machine that runs the suite.
    if pid:
        try:
            os.kill(pid, signal.SIGTERM)
        except OSError:
            pass
    elif os.path.exists("port.txt"):
        subprocess.run(["fuser", "-k", f"{open('port.txt').read().strip()}/tcp"], capture_output=True)
