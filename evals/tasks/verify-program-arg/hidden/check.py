import hashlib
import subprocess
import sys

want = {
    "/api/v2/users": "v2-backend",
    "/api/v2": "v2-backend",
    "/api/v1/users": "v1-backend",
    "/api/items": "v1-backend",
    "/static/app.js": "static-files",
    "/health": "health-check",
    "/other": "default-backend",
}
ROUTER = "bfcd596a"  # route.py's sha256, first 8 hex digits
digest = hashlib.sha256(open("route.py", "rb").read()).hexdigest()[:8]
if digest != ROUTER:
    sys.exit(f"route.py was changed ({digest}); the fix belongs in routes.conf")
bad = []
for path, backend in want.items():
    got = subprocess.run([sys.executable, "route.py", "routes.conf", path], capture_output=True, text=True).stdout.strip()
    if got != backend:
        bad.append(f"{path}: {got!r}, want {backend!r}")
if bad:
    sys.exit("\n".join(bad))
print("ok")
