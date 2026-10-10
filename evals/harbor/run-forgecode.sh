#!/usr/bin/env bash
# Run Terminal-Bench on Forge Code (forgecode.dev) through Harbor, the counterpart of run.sh.
#
#   evals/harbor/run-forgecode.sh smoke    # 1 task: does it start and reach the endpoint?
#   evals/harbor/run-forgecode.sh subset   # the first 10 tasks, 1 attempt
#   evals/harbor/run-forgecode.sh full     # every task, 5 attempts
#
# Settings (environment):
#   FORGE_OPENAI_BASE_URL, FORGE_OPENAI_API_KEY  the endpoint and key, as for ForgeCLI
#   FORGE_HARBOR_MODEL    the model, as the endpoint names it (default deep-thinking); the same
#                         variable sets it for run.sh, run-forgecode.sh and run-ante.sh
#   FORGE_HARBOR_DATASET  default terminal-bench@2.0
#   FORGE_HARBOR_JOBS     tasks in parallel (default 2)
#   FORGE_HARBOR_CA_BUNDLE   extra CAs to trust in containers (job name gets -hostca)
#   FORGECODE_BIN         the static Forge Code binary (default ~/.forgecode/forge, downloaded if missing)
#   FORGECODE_HARBOR_ARGS extra Forge Code flags for every task
# Anything after the mode goes to `harbor run` (e.g. -i 'some-task*').
set -euo pipefail
repo="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$repo"
mode="${1:-subset}"
shift || true

: "${FORGE_OPENAI_BASE_URL:?source your env file first}"
: "${FORGE_OPENAI_API_KEY:?source your env file first}"

bin="${FORGECODE_BIN:-$HOME/.forgecode/forge}"
if [ ! -x "$bin" ]; then
  echo "downloading Forge Code (forge-x86_64-unknown-linux-musl, latest release) to $bin"
  mkdir -p "$(dirname "$bin")"
  curl -fsSL -o "$bin" https://github.com/antinomyhq/forge/releases/latest/download/forge-x86_64-unknown-linux-musl
  chmod 755 "$bin"
fi
export FORGECODE_BIN="$bin"
version="$("$bin" --version 2>/dev/null | head -1 | tr -c 'A-Za-z0-9._-' '-' | sed 's/-*$//')"

case "$mode" in
  smoke) scope=(-l 1 --n-attempts 1) ;;
  subset) scope=(-l 10 --n-attempts 1) ;;
  full) scope=(--n-attempts 5) ;;
  *) echo "usage: $0 smoke|subset|full [harbor run args...]" >&2; exit 2 ;;
esac

ca=""
[ -n "${FORGE_HARBOR_CA_BUNDLE:-}" ] && ca="-hostca"
model="${FORGE_HARBOR_MODEL:-deep-thinking}"
model_tag="$(printf '%s' "${model#*/}" | tr -c 'A-Za-z0-9._-' '-')"
job="forgecode-${mode}-${model_tag}-${version:-unknown}${ca}-$(date +%Y%m%d-%H%M%S)"
echo "job: $job"

# The report is written even when harbor fails part way.
status=0
PYTHONPATH="$repo/evals/harbor${PYTHONPATH:+:$PYTHONPATH}" harbor run \
  -d "${FORGE_HARBOR_DATASET:-terminal-bench@2.0}" \
  -m "$model" \
  --agent forgecode_agent:ForgeCode \
  --n-concurrent "${FORGE_HARBOR_JOBS:-2}" \
  --job-name "$job" \
  "${scope[@]}" "$@" || status=$?
python3 "$repo/evals/harbor/report.py" "jobs/$job" || true
exit "$status"
