python3 - <<'PY'
s = open("invoice.py").read()
s = s.replace("    return sub + sub * tax_rate", "    return round(sub + sub * tax_rate, 2)")
open("invoice.py", "w").write(s)
PY
