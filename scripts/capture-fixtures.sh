#!/usr/bin/env bash
# Capture real stream-json transcripts from the reference CLI so the
# `inferred` rows in docs/PARITY.md can be checked against observed output.
#
# Run this on your own machine, with your own credentials:
#   REFERENCE_CLI=/path/to/reference-binary scripts/capture-fixtures.sh
#
# Output goes to docs/fixtures/observed/. Review it before committing: it
# contains your working directory and session ids, and must not contain keys.
set -euo pipefail
: "${REFERENCE_CLI:?set REFERENCE_CLI to the reference CLI binary}"
out="$(cd "$(dirname "$0")/.." && pwd)/docs/fixtures/observed"
mkdir -p "$out"
work="$(mktemp -d)"
cd "$work"
echo "hello" > notes.txt

# 1. One-shot print mode.
"$REFERENCE_CLI" -p --output-format stream-json --verbose "Reply with the single word: hi" > "$out/print-basic.jsonl"

# 2. Streaming input with partial messages and replayed user messages.
printf '%s\n' '{"type":"user","message":{"role":"user","content":"Reply with the single word: ok"},"parent_tool_use_id":null,"session_id":""}' |
  "$REFERENCE_CLI" -p --input-format stream-json --output-format stream-json --verbose \
    --include-partial-messages --replay-user-messages > "$out/stream-input.jsonl"

# 3. A permission prompt answered by a host (allow), via the stdio prompt channel.
python3 - "$REFERENCE_CLI" "$out/permission-prompt.jsonl" <<'PY'
import json, subprocess, sys
cli, dest = sys.argv[1], sys.argv[2]
p = subprocess.Popen([cli, "-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose",
                      "--permission-prompt-tool", "stdio"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
def send(o): p.stdin.write(json.dumps(o) + "\n"); p.stdin.flush()
send({"type": "control_request", "request_id": "init", "request": {"subtype": "initialize"}})
send({"type": "user", "message": {"role": "user", "content": "Run the shell command `touch made.txt` with your Bash tool, then say done."}, "parent_tool_use_id": None, "session_id": ""})
lines = []
for line in p.stdout:
    lines.append(line)
    m = json.loads(line)
    if m.get("type") == "control_request" and m["request"].get("subtype") == "can_use_tool":
        send({"type": "control_response", "response": {"subtype": "success", "request_id": m["request_id"],
              "response": {"behavior": "allow", "updatedInput": m["request"]["input"]}}})
    if m.get("type") == "result":
        break
p.stdin.close(); p.wait(timeout=60)
open(dest, "w").writelines(lines)
PY
echo "Captured into $out"
