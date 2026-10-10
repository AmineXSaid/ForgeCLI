#!/bin/sh
# Create the scratch repository that docs/CHECKLIST.md runs in.
#
#   scripts/check-setup.sh [DIR] [--force]
#
# DIR defaults to /tmp/forge-check. A non-empty DIR is left alone unless
# --force is given, which empties it first. Rerun with --force to reset
# between parts of the checklist.
set -eu

dir=/tmp/forge-check
force=0
for arg in "$@"; do
  case "$arg" in
    --force) force=1 ;;
    -h|--help)
      sed -n '2,8p' "$0" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    -*)
      echo "check-setup: unknown option $arg" >&2
      exit 2
      ;;
    *) dir=$arg ;;
  esac
done

if [ -d "$dir" ] && [ -n "$(ls -A "$dir" 2>/dev/null)" ]; then
  if [ "$force" -ne 1 ]; then
    echo "check-setup: $dir is not empty; pass --force to replace its contents" >&2
    exit 1
  fi
  case "$dir" in
    /|"$HOME"|.|..) echo "check-setup: refusing to empty $dir" >&2; exit 1 ;;
  esac
  find "$dir" -mindepth 1 -maxdepth 1 -exec rm -rf {} +
fi
mkdir -p "$dir"
cd "$dir"

git init -q
git config user.email check@forge.invalid
git config user.name "Forge check"

# The pair most checks use: the committed add() is right...
printf 'def add(a, b):\n    return a + b\n' > calc.py
# The test runs with pytest or plain python3.
printf 'from calc import add\n\ndef test_add():\n    assert add(2, 3) == 5\n\nif __name__ == "__main__":\n    test_add()\n    print("ok")\n' > test_calc.py
# ...a file for @ mentions, and one a deny rule keeps out.
printf '# Release notes\n\nThe parser ships on Friday; the CLI on Monday.\n' > notes.md
printf 'the vault code is 4417\n' > secret.txt
# A project hook, so the /hooks screen has something to list.
mkdir -p .forge
printf '%s\n' '{"hooks": {"SessionStart": [{"hooks": [{"type": "command", "command": "true"}]}]}}' > .forge/settings.json
printf 'secret.txt\nother-repo/\n' > .gitignore
git add calc.py test_calc.py notes.md .forge/settings.json .gitignore
git commit -qm "Start: add() works"

# A branch with a planted bug, for /code-review.
git checkout -qb planted-bug
printf 'def add(a, b):\n    return a - b\n' > calc.py
git commit -qam "Planted bug"
git checkout -q -

# The working tree starts with the bug uncommitted: the tests fail, and /diff has something to show.
printf 'def add(a, b):\n    return a - b\n' > calc.py

# A second repository for /cd.
mkdir other-repo
(cd other-repo && git init -q && printf 'print("other")\n' > main.py)

echo "Ready: $dir"
echo "  calc.py has an uncommitted bug (add returns a - b); branch planted-bug commits the same bug."
echo "  notes.md is for @ mentions; secret.txt for the deny-rule check; other-repo/ for /cd."
