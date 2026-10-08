#!/usr/bin/env bash
# Build a fully static `forge` (musl) for Terminal-Bench task containers, which
# run many different Linux images. Uses Docker, so no musl toolchain is needed
# on the host.
#
#   evals/harbor/build-static.sh
#
# Output: target-static/release/forge
set -euo pipefail
repo="$(cd "$(dirname "$0")/../.." && pwd)"
docker run --rm -v "$repo":/src -w /src -e CARGO_TARGET_DIR=/src/target-static \
  rust:alpine sh -c "apk add --no-cache build-base >/dev/null && cargo build --release --bin forge"
bin="$repo/target-static/release/forge"
file "$bin" | grep -q 'static' || { echo "error: $bin is not statically linked" >&2; exit 1; }
"$bin" --version
