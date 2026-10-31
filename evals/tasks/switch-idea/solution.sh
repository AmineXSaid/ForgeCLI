python3 - <<'PY'
s = open("hashmap.py").read()
s = s.replace(
    "        return sum(ord(c) for c in key) % self.size",
    "        h = 2166136261\n        for c in key.encode():\n            h = ((h ^ c) * 16777619) & 0xFFFFFFFF\n        return h % self.size",
)
open("hashmap.py", "w").write(s)
PY
