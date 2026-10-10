#!/usr/bin/env bash
# Run Terminal-Bench on ForgeCLI through Harbor.
#
#   evals/harbor/run.sh smoke    # 1 task: does it start and reach the endpoint?
#   evals/harbor/run.sh subset   # the first 10 tasks, 1 attempt: does it work?
#   evals/harbor/run.sh full     # every task, 5 attempts: the leaderboard setup
#
# Settings (environment):
#   FORGE_HARBOR_MODEL    the model, as the endpoint names it (default deep-thinking); the same
#                         variable sets it for run.sh, run-forgecode.sh and run-ante.sh
#   FORGE_HARBOR_DATASET  default terminal-bench@2.0
#   FORGE_HARBOR_JOBS     tasks in parallel (default 2)
#   FORGE_HARBOR_ARGS     extra forge flags for every task
#   FORGE_HARBOR_AUTONOMOUS  0 runs forge without --autonomous (job name gets -noauto)
#   FORGE_HARBOR_TIME_LIMIT  seconds for --max-time (default: the task's own limit)
#   FORGE_HARBOR_CA_BUNDLE   extra CAs to trust in containers (job name gets -hostca)
#   FORGE_STATIC_BIN      use this binary as is (job name gets -custombin); by default
#                         target-static/release/forge is rebuilt when the source changed
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
# The binary must be built from the source the job is named after: rebuild it when it isn't.
custom=""
if [ -n "${FORGE_STATIC_BIN:-}" ]; then
  [ -x "$FORGE_STATIC_BIN" ] || { echo "error: FORGE_STATIC_BIN: $FORGE_STATIC_BIN not found" >&2; exit 1; }
  custom="-custombin"  # someone else's build: the job name can't vouch for it
else
  bin="target-static/release/forge"
  want="$(evals/harbor/build-static.sh --source-id)"
  have="$(cat "$bin.source" 2>/dev/null || echo none)"
  if [ ! -x "$bin" ] || [ "$have" != "$want" ]; then
    echo "the static forge binary was built from other source ($have, now $want): rebuilding"
    evals/harbor/build-static.sh
  fi
fi

case "$mode" in
  smoke) scope=(-l 1 --n-attempts 1) ;;
  subset) scope=(-l 10 --n-attempts 1) ;;
  full) scope=(--n-attempts 5) ;;
  *) echo "usage: $0 smoke|subset|full [harbor run args...]" >&2; exit 2 ;;
esac

commit="$(git rev-parse --short HEAD)"
dirty=""
git diff --quiet HEAD -- crates Cargo.toml Cargo.lock || dirty="-dirty"
# Runs with extra trusted CAs change the task environment: label them, they're for local comparisons.
ca=""
[ -n "${FORGE_HARBOR_CA_BUNDLE:-}" ] && ca="-hostca"
case "${FORGE_HARBOR_AUTONOMOUS:-1}" in 0|false|no|off) ca="${ca}-noauto" ;; esac
model="${FORGE_HARBOR_MODEL:-deep-thinking}"
model_tag="$(printf '%s' "${model#*/}" | tr -c 'A-Za-z0-9._-' '-')"
job="forgecli-${mode}-${model_tag}-${commit}${dirty}${custom}${ca}-$(date +%Y%m%d-%H%M%S)"
echo "job: $job"

# The report is written even when harbor fails part way.
status=0
PYTHONPATH="$repo/evals/harbor${PYTHONPATH:+:$PYTHONPATH}" harbor run \
  -d "${FORGE_HARBOR_DATASET:-terminal-bench@2.0}" \
  -m "$model" \
  --agent forge_agent:ForgeCLI \
  --n-concurrent "${FORGE_HARBOR_JOBS:-2}" \
  --job-name "$job" \
  "${scope[@]}" "$@" || status=$?
python3 "$repo/evals/harbor/report.py" "jobs/$job" || true
exit "$status"
