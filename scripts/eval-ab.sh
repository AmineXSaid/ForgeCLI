#!/usr/bin/env bash
# Measure a change: the same kodact-eval tasks and model against two builds, then the A/B table.
#
#   scripts/eval-ab.sh [--before COMMIT] [--label NAME] [--repeat N] [--only TASK]... -- KODACT_ARGS
#
#   source ~/.kodact-env
#   KODACT_CONTEXT_WINDOW=1000000 scripts/eval-ab.sh --before 078db8b --label large -- \
#     --model <a model with a large window> --dangerously-skip-permissions
#
# COMMIT (default: the parent of HEAD) is built in a git worktree under target/ab/, HEAD here.
# Both run HEAD's evals/tasks, with the endpoint, key and window of your environment: set
# KODACT_CONTEXT_WINDOW to what the model really serves. Each run's reports land in new
# directories, evals/results/<NAME>-<time>-before and -after (git ignores evals/results/), so an
# earlier run is never overwritten; each holds ab.txt with the commit, the window and the
# arguments. The table compares B (HEAD) with A.
# docs/BASELINE.md says how to read it: with one run per task, a few runs either way is noise.
set -euo pipefail
repo="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo"

before="HEAD~1"
label=ab
repeat=1
only=()
while [ $# -gt 0 ]; do
  case "$1" in
    --before) before="$2"; shift 2 ;;
    --label) label="$2"; shift 2 ;;
    --repeat) repeat="$2"; shift 2 ;;
    --only) only+=(--only "$2"); shift 2 ;;
    --) shift; break ;;
    -h | --help) awk 'NR > 1 && /^#/ { sub(/^# ?/, ""); print; next } NR > 1 { exit }' "$0"; exit 0 ;;
    *) echo "eval-ab: unknown option $1 (try --help)" >&2; exit 2 ;;
  esac
done
if [ $# -eq 0 ]; then
  echo "eval-ab: give kodact's arguments after --, at least --model" >&2
  exit 2
fi
if [ -z "${KODACT_OPENAI_BASE_URL:-}${KODACT_BASE_URL:-}${KODACT_API_KEY:-}${KODACT_AUTH_TOKEN:-}${KODACT_GEMINI_API_KEY:-}" ]; then
  echo "eval-ab: no endpoint or key in the environment: source your env file first" >&2
  exit 1
fi
if [ -z "${KODACT_CONTEXT_WINDOW:-}" ]; then
  echo "eval-ab: note: KODACT_CONTEXT_WINDOW is not set, so KodAct uses what it knows of the model (200,000 when it knows nothing)" >&2
fi

a="$(git rev-parse --short "$before^{commit}")"
b="$(git rev-parse --short HEAD)"
if ! git diff --quiet HEAD -- crates Cargo.toml Cargo.lock; then b="$b-dirty"; fi
wt="$repo/target/ab/$a"
if [ ! -d "$wt" ]; then git worktree add --detach "$wt" "$a" >/dev/null; fi
echo "eval-ab: building A ($a) and B ($b)"
(cd "$wt" && cargo build --release -q -p kodact-cli)
cargo build --release -q -p kodact-cli -p kodact-eval

stamp="$(date +%Y%m%d-%H%M%S)"
run() { # side, commit, binary
  local out="evals/results/$label-$stamp-$1"
  mkdir -p "$out"
  {
    echo "side: $1"
    echo "commit: $2"
    echo "window: ${KODACT_CONTEXT_WINDOW:-(not set)}"
    echo "args: ${kodact_args[*]}"
    echo "repeat: $repeat"
  } >"$out/ab.txt"
  echo "eval-ab: running $1 ($2)"
  target/release/kodact-eval run --label "$label-$stamp-$1" --out "$out" --kodact "$3" --repeat "$repeat" \
    ${only[@]+"${only[@]}"} -- "${kodact_args[@]}" >"$out/run.log" 2>&1 || {
    tail -n 20 "$out/run.log" >&2
    return 1
  }
}
kodact_args=("$@")
run before "$a" "$wt/target/release/kodact"
run after "$b" "$repo/target/release/kodact"
target/release/kodact-eval compare "evals/results/$label-$stamp-before" "evals/results/$label-$stamp-after"
echo "eval-ab: reports in evals/results/$label-$stamp-before and -after"
