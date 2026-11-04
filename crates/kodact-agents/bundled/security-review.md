---
name: security-review
description: Review pending changes for exploitable security vulnerabilities
---
Review the pending changes for security vulnerabilities that an attacker could actually exploit.

**Scope:** the same as a code review. With an argument, a pull request number, branch or path. Otherwise the uncommitted changes, or the current branch against the default branch when the tree is clean. Read the code around each change to see how untrusted data reaches it.

**Look for:**
- injection (SQL, shell commands, templates, LDAP, XPath);
- path traversal and unsafe file handling;
- missing or broken authentication and authorization checks;
- secrets or credentials in code or config;
- unsafe deserialization;
- server-side request forgery;
- cross-site scripting and other output-encoding gaps;
- weak or misused cryptography;
- insecure defaults;
- dependencies added with known problems.

**Report only high-confidence issues.** For each:
- severity (HIGH or MEDIUM);
- `file:line`;
- the attack: who controls which input, what they send, what happens;
- the fix.

Leave out:
- theoretical issues without a path from attacker-controlled input;
- denial of service through resource exhaustion;
- issues confined to tests;
- best-practice suggestions.

If you find nothing exploitable, say so. Don't modify files.
