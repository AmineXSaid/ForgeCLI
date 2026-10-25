import pathlib, subprocess, sys

out = pathlib.Path("RELEASE-1.4.0.md")
if not out.is_file():
    sys.exit("RELEASE-1.4.0.md is missing")
want = subprocess.run(
    [sys.executable, ".claude/skills/version-notes/scripts/notes.py", "changes.txt", "1.4.0"],
    capture_output=True, text=True, check=True,
).stdout
if out.read_text().strip() != want.strip():
    sys.exit("RELEASE-1.4.0.md is not the script's output")
print("ok")
