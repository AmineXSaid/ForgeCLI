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
# --network host: the container resolves names like this machine does (Docker's own DNS
# often fails on company networks). The forge-cargo volume keeps downloaded crates between
# builds. The container runs as root: it writes forge.source itself and gives target-static
# back to the user who ran this script.
docker run --rm --network host -v "$repo":/src -w /src -v forge-cargo:/usr/local/cargo/registry \
  -e CARGO_TARGET_DIR=/src/target-static rust:alpine sh -c "
    apk add --no-cache build-base >/dev/null &&
    cargo build --release --bin forge &&
    echo $id > /src/target-static/release/forge.source &&
    chown -R $(id -u):$(id -g) /src/target-static"
bin="$repo/target-static/release/forge"
file "$bin" | grep -q 'static' || { echo "error: $bin is not statically linked" >&2; exit 1; }
"$bin" --version
