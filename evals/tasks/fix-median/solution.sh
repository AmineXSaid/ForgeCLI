python3 - <<'PY'
import re
s = open("stats.py").read()
s = s.replace("""    if len(ordered) % 2 == 0:
        return ordered[mid]""", """    if len(ordered) % 2 == 0:
        return (ordered[mid - 1] + ordered[mid]) / 2""")
open("stats.py", "w").write(s)
PY
