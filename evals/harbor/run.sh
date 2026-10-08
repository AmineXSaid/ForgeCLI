#!/usr/bin/env bash
# Run Terminal-Bench on ForgeCLI through Harbor.
#
#   evals/harbor/run.sh subset   # the first 10 tasks, 1 attempt: does it work?
#   evals/harbor/run.sh full     # every task, 5 attempts: the leaderboard setup
#
# Settings (environment):
#   FORGE_HARBOR_MODEL    harbor model, provider/name (default openai/deep-thinking)
#   FORGE_HARBOR_DATASET  default terminal-bench@2.0
#   FORGE_HARBOR_JOBS     tasks in parallel (default 2)
#   FORGE_HARBOR_ARGS     extra forge flags for every task
#   FORGE_HARBOR_TIME_LIMIT  seconds for --max-time (default: the task's own limit)
#   FORGE_HARBOR_CA_BUNDLE   extra CAs to trust in containers (job name gets -hostca)
#   FORGE_*               endpoint, key and limits, passed into the containers
# Anything after the mode goes to `harbor run` (e.g. -i 'some-task*').
set -euo pipefail
repo="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$repo"
mode="${1:-subset}"
shift || true

if [ -z "${FORGE_OPENAI_BASE_URL:-}${FORGE_BASE_URL:-}${FORGE_API_KEY:-}${FORGE_AUTH_TOKEN:-}" ]; then
  echo "error: no FORGE_* endpoint or key set; source your env file first" >&2
  exit 1
fi
[ -x "${FORGE_STATIC_BIN:-target-static/release/forge}" ] || {
  echo "error: no static forge binary; run evals/harbor/build-static.sh" >&2
  exit 1
}

case "$mode" in
  subset) scope=(-l 10 --n-attempts 1) ;;
  full) scope=(--n-attempts 5) ;;
  *) echo "usage: $0 subset|full [harbor run args...]" >&2; exit 2 ;;
esac

commit="$(git rev-parse --short HEAD)"
dirty=""
git diff --quiet HEAD -- crates Cargo.toml Cargo.lock || dirty="-dirty"
# Runs with extra trusted CAs change the task environment: label them, they're for local comparisons.
ca=""
[ -n "${FORGE_HARBOR_CA_BUNDLE:-}" ] && ca="-hostca"
job="forgecli-${mode}-${commit}${dirty}${ca}-$(date +%Y%m%d-%H%M%S)"
echo "job: $job"

PYTHONPATH="$repo/evals/harbor${PYTHONPATH:+:$PYTHONPATH}" harbor run \
  -d "${FORGE_HARBOR_DATASET:-terminal-bench@2.0}" \
  -m "${FORGE_HARBOR_MODEL:-openai/deep-thinking}" \
  --agent forge_agent:ForgeCLI \
  --n-concurrent "${FORGE_HARBOR_JOBS:-2}" \
  --job-name "$job" \
  "${scope[@]}" "$@"
