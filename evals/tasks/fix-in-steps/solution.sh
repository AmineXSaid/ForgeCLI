python3 - <<'PY'
s = open("textstats.py").read()
s = s.replace('return len(text.split(" "))', "return len(words(text))")
s = s.replace("return sum(len(w) for w in ws) / len(text)", "return sum(len(w) for w in ws) / len(ws) if ws else 0.0")
s = s.replace("return Counter(words(text)).most_common(1)[0][0]", "return Counter(w.lower() for w in words(text)).most_common(1)[0][0]")
open("textstats.py", "w").write(s)
PY
