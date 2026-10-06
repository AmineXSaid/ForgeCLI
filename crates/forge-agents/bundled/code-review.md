---
name: code-review
description: Review code changes for bugs, with a depth level and an optional fix pass
---
Review code changes and report real problems, most severe first.

**Arguments** (all optional, any order): a depth (`low`, `medium`, `high` or `max`; default `medium`), `--fix`, and what to review: a pull request number, a branch, or a path.

**What to review:**
- A pull request number: its diff (`gh pr diff <n>`, when the gh command is available).
- A branch: `git diff <default branch>...<branch>`.
- A path: that file or directory as it is now.
- Nothing given: the uncommitted changes (`git diff HEAD`, plus untracked files). If the tree is clean, the current branch against the default branch.

**How deep:**
- `low`: one pass for correctness bugs in the changed lines.
- `medium`: also the code the change calls and is called from, error handling, edge cases and missing tests.
- `high`: also design, concurrency, performance and compatibility.
- `max`: as `high`, and split the review across sub-agents (the Task tool), one per area or group of files, run in parallel. Then check every finding they report yourself.

**Every finding must survive a check.** Read the surrounding code and confirm the problem is real before reporting it. Drop anything speculative, style-only (unless the project's instructions require it) or already handled elsewhere.

**Report** each finding as:
- severity (high, medium or low), with `file:line`;
- what is wrong, in one sentence;
- a concrete input or state that triggers it;
- the fix.

If nothing survives, say so plainly.

**With `--fix`:** after the report, fix the confirmed findings, run the project's checks, and report what changed. Without `--fix`, don't modify any files.
