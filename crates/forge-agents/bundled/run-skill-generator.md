---
name: run-skill-generator
description: Write a project skill that captures how to build, run and check this project
disable-model-invocation: true
---
Write `.forge/skills/run/SKILL.md` for this project, so later sessions can start and check it without rediscovering how.

1. Find the working commands: install, build, run (and how to tell it is ready), the main thing to try once it runs, tests, and how to stop it. Confirm each by running it where that is safe and quick.
2. Write the skill:
   - frontmatter with `name: run` and a one-line `description`;
   - numbered steps with exact commands;
   - the ports, environment variables and services it needs;
   - known pitfalls you hit.
   Keep it under 60 lines.
3. Show the file. Tell the user that `/run` now uses it, and that they may want to commit it.

Don't record secrets or machine-specific paths in it.
