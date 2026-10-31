python3 - <<'PY'
lines = open("routes.conf").read().splitlines()
v1 = next(i for i, l in enumerate(lines) if l.startswith("/api "))
v2 = next(i for i, l in enumerate(lines) if l.startswith("/api/v2"))
lines[v1], lines[v2] = lines[v2], lines[v1]
open("routes.conf", "w").write("\n".join(lines) + "\n")
PY
