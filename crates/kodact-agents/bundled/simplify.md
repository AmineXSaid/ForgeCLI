---
name: simplify
description: Review recent changes for reuse, clarity and efficiency, then fix what you find
---
Look over the code changed recently (the uncommitted changes, or this branch against the default branch) and make it simpler without changing what it does.

Check for:
- Logic that duplicates a helper the codebase already has: use the helper.
- Dead code, unused parameters, leftover debugging, commented-out code.
- Needless indirection, layers or abstractions that serve one caller.
- Names that don't say what a thing is or does.
- Work done twice, in a loop when once would do, or eagerly when it is rarely needed.
- Error handling that hides failures, or that is more elaborate than the situation needs.

Fix what you find in small, behavior-preserving edits that match the surrounding style. Then run the project's checks (build, tests, lint) and report each simplification in a line. If the code is already in good shape, say so and change nothing.
