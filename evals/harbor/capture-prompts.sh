#!/usr/bin/env bash
# Save the exact system prompt and tool descriptions that ForgeCLI, Forge Code and Ante
# send to a model, for side-by-side comparison. Nothing leaves this machine: each agent
# talks to capture_prompt.py on localhost, which records the request and answers "Done.".
#
#   evals/harbor/capture-prompts.sh            # all three that are installed
#   evals/harbor/capture-prompts.sh ante       # one of: forgecli forgecode ante
#
# Output: evals/harbor/prompts/<harness>/system.md, tools.json, request-*.json, version.txt,
# and agent.log / server.log to see why when nothing was captured.
# Binaries: FORGECLI_BIN (target-static/release/forge, else target/release/forge), FORGECODE_BIN
# (~/.forgecode/forge), ANTE_BIN (~/.ante/bin/ante). Set CAPTURE_PORT to change the port.
set -uo pipefail
repo="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$repo"
port="${CAPTURE_PORT:-8765}"
url="http://127.0.0.1:$port/v1"
task="Reply with the single word: done."
out="evals/harbor/prompts"
which=("${@:-forgecli forgecode ante}")
read -r -a which <<< "${which[*]}"

capture() {  # name, binary, then the command to run with its environment
  local name="$1" bin="$2"; shift 2
  rm -rf "${out:?}/$name"
  mkdir -p "$out/$name"
  python3 evals/harbor/capture_prompt.py --out "$out/$name" --port "$port" --max-requests 3 2> "$out/$name/server.log" &
  local server=$!
  echo "== $name"
  # An agent that starts before the server listens fails at once, so wait for it.
  for _ in $(seq 50); do
    python3 -c "import sys,urllib.request; urllib.request.urlopen(sys.argv[1] + '/models', timeout=1)" "$url" 2>/dev/null && break
    sleep 0.2
  done
  (cd "$(mktemp -d)" && timeout 60 env "$@" > "$repo/$out/$name/agent.log" 2>&1)
  local status=$?
  kill "$server" 2>/dev/null
  wait "$server" 2>/dev/null
  "$bin" --version > "$out/$name/version.txt" 2>&1
  if [ -f "$out/$name/system.md" ]; then
    echo "   $out/$name/system.md: $(wc -w < "$out/$name/system.md") words; tools.json: $(python3 -c "import json,sys; print(len(json.load(open(sys.argv[1]))))" "$out/$name/tools.json") tools"
  else
    echo "   no request with tools captured (agent exit status $status); see $out/$name/agent.log and server.log"
  fi
}

for h in "${which[@]}"; do
  case "$h" in
    forgecli)
      bin="${FORGECLI_BIN:-target-static/release/forge}"; [ -x "$bin" ] || bin="target/release/forge"
      [ -x "$bin" ] || { echo "== forgecli: no binary (build it first)"; continue; }
      case "$bin" in /*) ;; *) bin="$repo/$bin" ;; esac
      capture forgecli "$bin" FORGE_OPENAI_BASE_URL="$url" FORGE_OPENAI_API_KEY=capture \
        "$bin" -p "$task" --model capture --autonomous --dangerously-skip-permissions --output-format stream-json --verbose ;;
    forgecode)
      bin="${FORGECODE_BIN:-$HOME/.forgecode/forge}"
      [ -x "$bin" ] || { echo "== forgecode: $bin not found"; continue; }
      cfg="$(mktemp -d)"
      capture forgecode "$bin" OPENAI_URL="$url" OPENAI_API_KEY=capture FORGE_CONFIG="$cfg" \
        FORGE_SESSION__PROVIDER_ID=openai_compatible FORGE_SESSION__MODEL_ID=capture FORGE_UPDATES__AUTO_UPDATE=false \
        "$bin" -p "$task" ;;
    ante)
      bin="${ANTE_BIN:-$HOME/.ante/bin/ante}"
      [ -x "$bin" ] || { echo "== ante: $bin not found"; continue; }
      capture ante "$bin" OPENAI_COMPATIBLE_BASE_URL="$url" OPENAI_COMPATIBLE_API_KEY=capture \
        "$bin" --provider openai-compatible --model capture --yolo --no-session-save --no-skills -p "$task" ;;
    *) echo "unknown harness: $h (forgecli, forgecode, ante)" >&2 ;;
  esac
done
