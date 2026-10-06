python3 - <<'PY'
data = open("config.ini", "rb").read()
data = data.replace(b"host = example.org\r\ntimeout = 30\r\n", b"host = example.org\r\ntimeout = 45\r\n", 1)
open("config.ini", "wb").write(data)
PY
