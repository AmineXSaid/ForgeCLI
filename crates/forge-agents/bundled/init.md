---
name: init
description: Write or improve FORGE.md, the project's instructions for Forge
disable-model-invocation: true
---
Write a FORGE.md at the project root that will make future sessions in this repository productive from the first prompt. If one exists, improve it instead of starting over, and say what you changed.

Investigate before writing:
- How to build, test and lint, including how to run a single test. Prefer commands you have confirmed in the manifests (package.json scripts, Makefile, Cargo.toml, pyproject.toml, go.mod and so on) or by running them.
- The architecture that takes several files to understand: the main components, how a request or command flows through them, where the important boundaries are.
- Conventions the code follows that a newcomer would get wrong: error handling, naming, test layout, generated files that must not be edited by hand.
- Instructions already written for other agents or editors (AGENTS.md, GEMINI.md, .cursor/rules, .github/copilot-instructions.md and similar) and the useful parts of the README. Fold in what still holds.

Write it for an engineer who is new to the repository:
- Short, specific and true. No generic advice ("write tests", "keep functions small") and no file listings anyone can get with ls.
- Don't invent facts. If something is unclear, leave it out or mark it as a question for the user.
- Aim for under 150 lines. Use headings and lists; put commands in code blocks.

Finish with a two-line summary of what the file covers.
