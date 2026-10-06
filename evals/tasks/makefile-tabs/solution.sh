python3 - <<'PY'
s = open("Makefile").read()
open("Makefile", "w").write(s.replace("\tpython3 -m unittest -q\n", "\tpython3 -m unittest -v\n"))
PY
