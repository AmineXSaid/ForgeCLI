---
name: version-notes
description: Turn a changes file into release notes for a version
argument-hint: <version> <changes-file>
arguments: [version, changes]
allowed-tools: Bash(python3 ${CLAUDE_SKILL_DIR}/scripts/notes.py *)
---
Write the release notes for version $version.

1. Run `python3 ${CLAUDE_SKILL_DIR}/scripts/notes.py $ARGUMENTS[1] $0` and
   save exactly what it prints to `RELEASE-$0.md` in ${CLAUDE_PROJECT_DIR}.
2. Do not edit the output by hand.
