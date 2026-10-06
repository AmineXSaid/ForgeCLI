---
name: fewer-permission-prompts
description: Cut down permission prompts by allowing safe commands this project uses often
disable-model-invocation: true
---
Propose permission rules that would stop routine, safe tool calls from asking for approval in this project. Add only the ones the user picks.

1. **Collect candidates:**
   - the shell commands used in this conversation and in recent sessions for this project (their transcripts are JSONL files under the projects directory in Forge's config directory);
   - the project's standard commands (test, build, lint and format scripts).
2. **Keep only safe, specific rules.** Good: read-only commands, and the project's own scripts with a fixed prefix (`Bash(npm run test:*)`, `Bash(cargo check:*)`, `Bash(git status)`, `Bash(git diff:*)`). Never: wildcards that allow arbitrary commands (`Bash(*)`, `Bash(sh:*)`, `Bash(python:*)`), anything that deletes, pushes, publishes, deploys, or downloads and runs code, or anything touching credentials.
3. **Ask.** Show the candidates with a one-line reason each, and ask the user (AskUserQuestion, multi-select) which to add and where: local settings, just for them (`.forge/settings.local.json`, the default), or project settings, shared (`.forge/settings.json`).
4. **Add the chosen rules** under `permissions.allow`, keeping what is already there. Read the file first and keep it valid JSON. Then list what you added.
