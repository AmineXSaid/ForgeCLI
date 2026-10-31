python3 - <<'PY'
s = open("shapes.py").read()
s = s.replace("return width + height", "return width * height")
s = s.replace("return math.pi * radius", "return math.pi * radius * radius")
open("shapes.py", "w").write(s)
PY
