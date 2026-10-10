#!/usr/bin/env bash
# Terminal-Bench on Ante CLI through Harbor (counterpart of run.sh).
#   FORGE_HARBOR_MODEL=deep-thinking evals/harbor/run-ante.sh smoke|subset|full [harbor args...]
# Settings: FORGE_HARBOR_MODEL (default deep-thinking, as for run.sh; ANTE_MODEL also works),
#           ANTE_PROVIDER (openai-compatible), ANTE_INSTALL_ARGS (empty = stable),
#           FORGE_HARBOR_JOBS (tasks in parallel, default 2), FORGE_HARBOR_DATASET,
#           FORGE_HARBOR_CA_BUNDLE (label only for now)
set -euo pipefail
repo="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$repo"
mode="${1:-subset}"; shift || true

: "${FORGE_OPENAI_BASE_URL:?source ~/.forge-env-kpit first}"
: "${FORGE_OPENAI_API_KEY:?source ~/.forge-env-kpit first}"

case "$mode" in
  smoke)  scope=(-l 1 --n-attempts 1) ;;
  subset) scope=(-l 10 --n-attempts 1) ;;
  full)   scope=(--n-attempts 5) ;;
  *) echo "usage: $0 smoke|subset|full [harbor run args...]" >&2; exit 2 ;;
esac

model="${FORGE_HARBOR_MODEL:-${ANTE_MODEL:-deep-thinking}}"
model_tag="$(printf '%s' "${model#*/}" | tr -c 'A-Za-z0-9._-' '-')"
provider="${ANTE_PROVIDER:-openai-compatible}"
install="${ANTE_INSTALL_ARGS:-}"
ca=""; [ -n "${FORGE_HARBOR_CA_BUNDLE:-}" ] && ca="-hostca"
job="ante-${mode}-${model_tag}-${install:-stable}${ca}-$(date +%Y%m%d-%H%M%S)"
echo "job: $job"

# The report is written even when harbor fails part way.
status=0
# The agent is evals/harbor/ante_agent.py; ~/ante-src/ante-harbor, if there, comes after it.
PYTHONPATH="$repo/evals/harbor:$HOME/ante-src/ante-harbor${PYTHONPATH:+:$PYTHONPATH}" harbor run \
  -d "${FORGE_HARBOR_DATASET:-terminal-bench@2.0}" \
  -m "$model" \
  --agent ante_agent:AnteAgent \
  --ak provider="$provider" --ak install_args="$install" \
  --ae "OPENAI_COMPATIBLE_BASE_URL=$FORGE_OPENAI_BASE_URL" \
  --ae "OPENAI_COMPATIBLE_API_KEY=$FORGE_OPENAI_API_KEY" \
  --n-concurrent "${FORGE_HARBOR_JOBS:-2}" \
  --job-name "$job" \
  "${scope[@]}" "$@" || status=$?
python3 "$repo/evals/harbor/report.py" "jobs/$job" || true
exit "$status"
