---
name: verify
description: Prove a change works by running it, not by reading it
---
Verify that the recent change works, with evidence.

1. Work out what changed and what it is supposed to do: the conversation, the diff, the task list.
2. Run the project's checks: build, tests and lint. Note the exact commands and results.
3. Exercise the behavior directly, the way a user would. Run the command, call the endpoint, or load the page; or write a short throwaway script that calls the changed code with realistic input, including an edge case. Remove scratch files afterwards.
4. Report:
   - what you verified and how;
   - what passed;
   - what failed, with the output;
   - what you could not verify, and why.

Don't call something verified that you didn't run. If a check fails, say so and stop; don't paper over it.
