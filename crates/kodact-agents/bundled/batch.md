---
name: batch
description: Make the same change across many files, in parallel sub-agents
disable-model-invocation: true
---
Apply the change described in the arguments across the codebase, using sub-agents in parallel.

1. **Find every site.** Use Grep and Glob to list each file or location the change applies to. Show the count, and a sample if there are many. If the instruction is ambiguous, ask before changing anything.
2. **Plan.** Group the sites into independent batches that don't touch the same files: about 5-15 files each, at most 8 batches. Write down the exact transformation once, with a before/after example, so every batch applies it the same way.
3. **Run the batches in parallel** with the Task tool. Give each sub-agent its file list, the transformation and the example, and tell it to edit only those files and report what it changed or skipped.
4. **Check the whole.** Build and run the tests, grep for sites that were missed or changed wrongly, and fix them.
5. **Report:** files changed, sites skipped and why, and the check results.
