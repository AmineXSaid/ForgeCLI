#!/usr/bin/env bash
# Build a fully static `forge` (musl) for Terminal-Bench task containers, which
# run many different Linux images. Uses Docker, so no musl toolchain is needed
# on the host.
#
#   evals/harbor/build-static.sh              # build
#   evals/harbor/build-static.sh --source-id  # print the id of the current source
#
# Output: target-static/release/forge, and forge.source next to it: the id of the
# source it was built from, which run.sh compares with the current one.
set -euo pipefail
repo="$(cd "$(dirname "$0")/../.." && pwd)"

# The committed crates and Cargo files, plus any uncommitted change to them.
source_id() {
  { git -C "$repo" rev-parse HEAD:crates HEAD:Cargo.toml HEAD:Cargo.lock
    git -C "$repo" diff HEAD -- crates Cargo.toml Cargo.lock
  } | sha1sum | cut -c1-12
}
if [ "${1:-}" = "--source-id" ]; then
  source_id
  exit 0
fi

id="$(source_id)"
docker run --rm -v "$repo":/src -w /src -e CARGO_TARGET_DIR=/src/target-static \
  rust:alpine sh -c "apk add --no-cache build-base >/dev/null && cargo build --release --bin forge"
bin="$repo/target-static/release/forge"
file "$bin" | grep -q 'static' || { echo "error: $bin is not statically linked" >&2; exit 1; }
echo "$id" > "$bin.source"
"$bin" --version
