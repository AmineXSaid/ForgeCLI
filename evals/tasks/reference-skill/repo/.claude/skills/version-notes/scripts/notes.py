import sys

changes, version = sys.argv[1], sys.argv[2]
lines = [l.strip() for l in open(changes) if l.strip()]
fixes = sorted(l[4:].strip() for l in lines if l.lower().startswith("fix:"))
feats = sorted(l[5:].strip() for l in lines if l.lower().startswith("feat:"))
print(f"# Release {version}\n")
print("## Features\n" + "\n".join(f"- {f}" for f in feats) + "\n")
print("## Fixes\n" + "\n".join(f"- {f}" for f in fixes))
