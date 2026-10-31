python3 - <<'PY'
s = open("wordcount.py").read()
s = s.replace('text.split(" ") if w != ""', 'text.split() if w != ""')
open("wordcount.py", "w").write(s)
PY
python3 -m unittest -q test_wordcount && git add wordcount.py && git commit -q -m "Count words separated by any whitespace"
