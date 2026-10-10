#!/usr/bin/env bash
# Terminal-Bench on Ante CLI through Harbor (counterpart of run.sh).
#   ANTE_MODEL=quick-thinking evals/harbor/run-ante.sh smoke|subset|full [harbor args...]
# Settings: ANTE_MODEL, ANTE_PROVIDER (openai-compatible), ANTE_INSTALL_ARGS (empty = stable),
#           FORGE_HARBOR_JOBS (tasks in parallel, default 2), FORGE_HARBOR_DATASET,
#           FORGE_HARBOR_CA_BUNDLE (label only for now)
set -euo pipefail
repo="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$repo"
mode="${1:-subset}"; shift || true

: "${FORGE_OPENAI_BASE_URL:?source ~/.forge-env-kpit first}"
: "${FORGE_OPENAI_API_KEY:?source ~/.forge-env-kpit first}"
[ -d "$HOME/ante-src/ante-harbor" ] || { echo "error: ~/ante-src/ante-harbor missing" >&2; exit 1; }

case "$mode" in
  smoke)  scope=(-l 1 --n-attempts 1) ;;
  subset) scope=(-l 10 --n-attempts 1) ;;
  full)   scope=(--n-attempts 5) ;;
  *) echo "usage: $0 smoke|subset|full [harbor run args...]" >&2; exit 2 ;;
esac

model="${ANTE_MODEL:-quick-thinking}"
provider="${ANTE_PROVIDER:-openai-compatible}"
install="${ANTE_INSTALL_ARGS:-}"
ca=""; [ -n "${FORGE_HARBOR_CA_BUNDLE:-}" ] && ca="-hostca"
job="ante-${mode}-${model}-${install:-stable}${ca}-$(date +%Y%m%d-%H%M%S)"
echo "job: $job"

PYTHONPATH="$HOME/ante-src/ante-harbor${PYTHONPATH:+:$PYTHONPATH}" harbor run \
  -d "${FORGE_HARBOR_DATASET:-terminal-bench@2.0}" \
  -m "$model" \
  --agent ante_agent:AnteAgent \
  --ak provider="$provider" --ak install_args="$install" \
  --ae "OPENAI_COMPATIBLE_BASE_URL=$FORGE_OPENAI_BASE_URL" \
  --ae "OPENAI_COMPATIBLE_API_KEY=$FORGE_OPENAI_API_KEY" \
  --n-concurrent "${FORGE_HARBOR_JOBS:-2}" \
  --job-name "$job" \
  "${scope[@]}" "$@"
