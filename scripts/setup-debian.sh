#!/usr/bin/env bash
# Set up Debian or Ubuntu (WSL included) to build and test KodAct and to run its Harbor
# benchmarks, without certificate errors or missing packages.
#
#   scripts/setup-debian.sh               # every step, then the checks
#   scripts/setup-debian.sh --check       # the checks only: changes nothing
#   scripts/setup-debian.sh --no-docker   # every step but Docker (cargo test doesn't need it)
#
# Source your env file first, so the checks cover your endpoint too. Each step keeps what is
# already in place, so the script is safe to run again. Every change goes where nothing rewrites
# it later, so it holds for every later build, test and Harbor run, after `wsl --shutdown`,
# `git pull` and package upgrades. Nothing goes into WSL's /etc/resolv.conf or /etc/hosts
# (rewritten at each start), into the certificate bundle itself (rebuilt by
# update-ca-certificates) or into this repository (replaced by git pull):
#   company CA     /usr/local/share/ca-certificates/kodact-company-ca-*.crt
#   tool settings  ~/.kodact-certs.sh, read by ~/.bashrc (this script's own file)
#   Docker DNS     the "dns" key of /etc/docker/daemon.json (other keys kept)
#   WSL systemd    "systemd=true" under [boot] in /etc/wsl.conf (other settings kept)
#   Rust, uv       ~/.cargo and ~/.local/bin (their installers put them on PATH)
# Your endpoint, key and run settings stay in your own env file: the script never writes it.
#
# Behind a proxy that inspects HTTPS (Zscaler and the like), sites arrive signed by the
# company's CA, which a fresh system doesn't trust. The script then takes that CA from Windows'
# trust store (under WSL) or from the file KODACT_CA_FILE names (PEM or DER), never from the
# network: it adds only certificates that Windows or that file already trusts, and only those
# that sign what the sites sent.
#
# Settings: KODACT_CA_FILE; KODACT_DOCKER_DNS ("10.0.0.1 10.0.0.2": DNS servers to try for
# containers); KODACT_HARBOR_VERSION (default 0.24.0, the version kodact_agent.py is written for).
set -euo pipefail

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BUNDLE=/etc/ssl/certs/ca-certificates.crt
MOZILLA=/usr/share/ca-certificates/mozilla
ANCHORS=/usr/local/share/ca-certificates
DAEMON_JSON=/etc/docker/daemon.json
WSL_CONF=/etc/wsl.conf
RESOLV_CONF=/etc/resolv.conf
CERTS_ENV="$HOME/.kodact-certs.sh"
BASHRC="$HOME/.bashrc"
SITES=(github.com pypi.org static.crates.io astral.sh download.docker.com)
# Names task containers look up: the tests fetch uv and packages.
CONTAINER_NAMES=(astral.sh pypi.org github.com)
PACKAGES=(build-essential pkg-config git curl ca-certificates openssl python3 python3-venv
  python3-pip python3-pytest bubblewrap file jq dnsutils procps)
HARBOR_VERSION="${KODACT_HARBOR_VERSION:-0.24.0}"
TEST_IMAGE=alpine:3.20
CURL_IMAGE=curlimages/curl:8.11.1

CHECK=false
DOCKER=true
SUDO=
[ "$(id -u)" -eq 0 ] || SUDO=sudo
DOCKER_CMD=(docker)
INTERCEPTED=false # a site's certificate chain needs a CA beyond the public ones
REACHED=false     # at least one site answered, so INTERCEPTED means something
CA_CHANGED=false
WORK=
failures=()
todos=()

step() { printf '\n== %s\n' "$*"; }
ok() { printf '   ok    %s\n' "$*"; }
fail() {
  printf '   FAIL  %s\n' "$*"
  failures+=("$*")
}
todo() {
  printf '   TODO  %s\n' "$*"
  todos+=("$*")
}
note() { printf '         %s\n' "$*"; }

# Runs a command with its output kept aside; on failure, shows the end of it.
run_logged() {
  local log="$WORK/log"
  if "$@" >"$log" 2>&1; then return 0; fi
  tail -n 12 "$log" | sed 's/^/         | /'
  return 1
}

is_wsl() { [ -n "${WSL_DISTRO_NAME:-}" ] || grep -qi microsoft /proc/sys/kernel/osrelease 2>/dev/null; }
has_systemd() { [ "$(cat /proc/1/comm 2>/dev/null)" = systemd ]; }

# Runs a PowerShell script on the Windows side of WSL and prints its output.
winps() {
  is_wsl || return 1
  local exe
  exe="$(command -v powershell.exe 2>/dev/null || echo /mnt/c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe)"
  [ -x "$exe" ] || return 1
  (
    cd /mnt/c 2>/dev/null || true # a Linux working directory makes Windows programs complain
    "$exe" -NoProfile -NonInteractive -EncodedCommand "$(printf '%s' "$1" | iconv -f UTF-8 -t UTF-16LE | base64 -w0)"
  ) 2>/dev/null | tr -d '\r'
}

# --- Certificates ---------------------------------------------------------------------------

# One file per certificate: $1 a PEM file, $2 the directory, $3 a name prefix.
split_pem() {
  awk -v d="$2" -v p="$3" '
    /-----BEGIN CERTIFICATE-----/ { n++; f = sprintf("%s/%s%03d.pem", d, p, n); on = 1 }
    on { print > f }
    /-----END CERTIFICATE-----/ { on = 0; close(f) }' "$1"
}

last_cert() {
  awk '/-----BEGIN CERTIFICATE-----/ { c = ""; on = 1 } on { c = c $0 "\n" }
    /-----END CERTIFICATE-----/ { on = 0; last = c } END { printf "%s", last }' "$1"
}
subject_of() { openssl x509 -noout -subject -nameopt oneline 2>/dev/null | sed 's/^subject= *//'; }
issuer_of() { openssl x509 -noout -issuer -nameopt oneline 2>/dev/null | sed 's/^issuer= *//'; }

# False when NO_PROXY names host $1.
uses_proxy() {
  local e list=()
  IFS=, read -ra list <<<"${NO_PROXY:-${no_proxy:-}}"
  for e in ${list[@]+"${list[@]}"}; do
    e="${e// /}"
    [ "$e" = "*" ] && return 1
    e="${e#\*}"
    e="${e#.}"
    [ -n "$e" ] || continue
    [ "$1" = "$e" ] && return 1
    case "$1" in *."$e") return 1 ;; esac
  done
  return 0
}

# The certificates site $1 sends (leaf first), through the HTTPS proxy when one applies.
presented_chain() {
  local proxy="${HTTPS_PROXY:-${https_proxy:-}}" px=()
  if [ -n "$proxy" ] && uses_proxy "$1"; then
    proxy="${proxy#*://}"
    proxy="${proxy##*@}"
    px=(-proxy "${proxy%%/*}")
  fi
  timeout 20 openssl s_client -connect "$1:443" -servername "$1" -showcerts ${px[@]+"${px[@]}"} \
    </dev/null 2>/dev/null | awk '/-----BEGIN CERTIFICATE-----/{on=1} on{print} /-----END CERTIFICATE-----/{on=0}' || true
}

# True when the chain in $2 (leaf first, as a site sent it) verifies against trust store $1.
chain_verifies() {
  local leaf="$WORK/leaf.pem"
  awk '/-----BEGIN CERTIFICATE-----/{n++} n==1{print} /-----END CERTIFICATE-----/{if (n==1) exit}' "$2" >"$leaf"
  [ -s "$leaf" ] || return 1
  openssl verify -no-CApath -no-CAstore -CAfile "$1" -untrusted "$2" "$leaf" >/dev/null 2>&1
}

# "subject-hash path" for each certificate file in directory $1.
index_candidates() {
  local f h
  for f in "$1"/*.pem; do
    [ -e "$f" ] || continue
    h="$(openssl x509 -in "$f" -noout -subject_hash 2>/dev/null)" || continue
    printf '%s %s\n' "$h" "$f"
  done
}

# The candidates (index $2) that complete chain $1: starting from the last certificate the site
# sent, the candidate whose subject is its issuer and whose key signed it, up to a self-signed root.
signers_for() {
  local dir cur h found hash c _
  dir="$(mktemp -d "$WORK/chain.XXXX")"
  split_pem "$1" "$dir" c
  cur="$(find "$dir" -name 'c*.pem' | sort | tail -n 1)"
  [ -n "$cur" ] || return 0
  for _ in 1 2 3 4 5 6; do
    h="$(openssl x509 -in "$cur" -noout -issuer_hash)"
    found=""
    while read -r hash c; do
      [ "$hash" = "$h" ] || continue
      if openssl verify -no-CApath -no-CAstore -partial_chain -CAfile "$c" "$cur" >/dev/null 2>&1; then
        found="$c"
        break
      fi
    done <"$2"
    [ -n "$found" ] || return 0
    printf '%s\n' "$found"
    # A self-signed root ends the chain.
    if [ "$(openssl x509 -in "$found" -noout -issuer_hash)" = "$h" ]; then return 0; fi
    cur="$found"
  done
}

# Adds each certificate (one PEM file each) to the system store, named by its fingerprint, so
# that update-ca-certificates keeps it in the bundle from now on.
install_cas() {
  local f fp name
  for f in "$@"; do
    fp="$(openssl x509 -in "$f" -noout -fingerprint -sha256 | sed 's/.*=//; s/://g' | tr 'A-F' 'a-f' | cut -c1-16)"
    name="$ANCHORS/kodact-company-ca-$fp.crt"
    [ -f "$name" ] && continue
    $SUDO install -D -m 0644 "$f" "$name"
    note "added $(subject_of <"$f")"
    note "  as $name"
    CA_CHANGED=true
  done
  if $CA_CHANGED; then $SUDO update-ca-certificates >/dev/null; fi
}

# Where trusted CAs come from: Windows' root stores (machine and user), or KODACT_CA_FILE.
ca_source() {
  local out="$1"
  if [ -n "${KODACT_CA_FILE:-}" ]; then
    if [ ! -r "$KODACT_CA_FILE" ]; then
      fail "KODACT_CA_FILE: can't read $KODACT_CA_FILE"
      return 1
    fi
    if grep -q 'BEGIN CERTIFICATE' "$KODACT_CA_FILE"; then
      tr -d '\r' <"$KODACT_CA_FILE" >"$out"
    elif ! openssl x509 -inform der -in "$KODACT_CA_FILE" -out "$out" 2>/dev/null; then
      fail "KODACT_CA_FILE: $KODACT_CA_FILE is neither PEM nor DER"
      return 1
    fi
    echo "KODACT_CA_FILE ($KODACT_CA_FILE)"
    return 0
  fi
  # shellcheck disable=SC2016 # PowerShell's own $_, not the shell's
  local ps='Get-ChildItem Cert:\LocalMachine\Root, Cert:\CurrentUser\Root | ForEach-Object {
    "-----BEGIN CERTIFICATE-----"; [Convert]::ToBase64String($_.RawData, "InsertLineBreaks"); "-----END CERTIFICATE-----" }'
  if winps "$ps" >"$out" && grep -q 'BEGIN CERTIFICATE' "$out"; then
    echo "Windows' trust store"
    return 0
  fi
  return 1
}

step_ca() {
  step "Company certificate authority"
  local site moz="$WORK/mozilla.pem" need=() all=() signers=() src
  if ! command -v openssl >/dev/null 2>&1; then
    fail "openssl is missing, so certificates can't be checked: sudo apt-get install openssl"
    return 0
  fi
  cat "$MOZILLA"/*.crt >"$moz" 2>/dev/null || true
  local judge=true
  grep -q 'BEGIN CERTIFICATE' "$moz" || judge=false # no list of public roots to compare with
  for site in "${SITES[@]}"; do
    presented_chain "$site" >"$WORK/$site.pem"
    if ! grep -q 'BEGIN CERTIFICATE' "$WORK/$site.pem"; then
      fail "$site: no TLS answer on port 443 (network, DNS or proxy settings)"
      continue
    fi
    REACHED=true
    if $judge && ! chain_verifies "$moz" "$WORK/$site.pem"; then INTERCEPTED=true; fi
    if chain_verifies "$BUNDLE" "$WORK/$site.pem"; then
      ok "$site: trusted"
    else
      need+=("$site")
    fi
  done
  if $INTERCEPTED && [ ${#need[@]} -eq 0 ]; then note "HTTPS here is re-signed by a company CA, which this system trusts"; fi
  [ ${#need[@]} -gt 0 ] || return 0
  if $CHECK; then
    for site in "${need[@]}"; do fail "$site: signed by $(last_cert "$WORK/$site.pem" | issuer_of), not trusted here"; done
    return 0
  fi
  if ! src="$(ca_source "$WORK/source.pem")"; then
    for site in "${need[@]}"; do fail "$site: signed by $(last_cert "$WORK/$site.pem" | issuer_of), not trusted here"; done
    note "Get that CA from IT, or export it from Windows (certmgr.msc > Trusted Root Certification"
    note "Authorities > the CA above > Export), then rerun with KODACT_CA_FILE=/path/to/that/file"
    return 0
  fi
  mkdir -p "$WORK/candidates"
  split_pem "$WORK/source.pem" "$WORK/candidates" w
  index_candidates "$WORK/candidates" >"$WORK/index"
  for site in "${need[@]}"; do
    mapfile -t signers < <(signers_for "$WORK/$site.pem" "$WORK/index")
    if [ ${#signers[@]} -gt 0 ] && cat "$BUNDLE" "${signers[@]}" >"$WORK/store.pem" &&
      chain_verifies "$WORK/store.pem" "$WORK/$site.pem"; then
      all+=("${signers[@]}")
    else
      fail "$site: signed by $(last_cert "$WORK/$site.pem" | issuer_of), which $src doesn't trust either"
    fi
  done
  [ ${#all[@]} -gt 0 ] || return 0
  mapfile -t all < <(printf '%s\n' "${all[@]}" | sort -u)
  note "taking the CA from $src:"
  install_cas "${all[@]}"
  for site in "${need[@]}"; do
    if chain_verifies "$BUNDLE" "$WORK/$site.pem"; then ok "$site: trusted now"; fi
  done
}

# Tools that bring their own certificate list use the system store instead.
step_certs_env() {
  step "Certificate settings for every tool ($CERTS_ENV)"
  local harbor_ca=false new="$WORK/certs.sh" v
  # Once set, the Harbor CA stays: a network that stops inspecting HTTPS for a while (another
  # network, the VPN off) must not change the next runs.
  if $INTERCEPTED || grep -q '^export KODACT_HARBOR_CA_BUNDLE=' "$CERTS_ENV" 2>/dev/null; then harbor_ca=true; fi
  {
    echo "# Written by KodAct's scripts/setup-debian.sh, which may rewrite it: keep your own settings elsewhere."
    echo "# Tools with their own certificate list (Python's requests and pip, uv, Node, cargo) use the"
    echo "# system store instead, which holds the company CA when there is one."
    for v in SSL_CERT_FILE REQUESTS_CA_BUNDLE CURL_CA_BUNDLE PIP_CERT NODE_EXTRA_CA_CERTS CARGO_HTTP_CAINFO GIT_SSL_CAINFO; do
      echo "export $v=$BUNDLE"
    done
    echo "export SSL_CERT_DIR=/etc/ssl/certs"
    echo "export UV_NATIVE_TLS=1 UV_SYSTEM_CERTS=1"
    if $harbor_ca; then
      echo "# HTTPS on this network is re-signed by a company CA: Harbor's task containers trust it too"
      echo "# (evals/harbor/README.md, \"Behind a company proxy\"; job names get -hostca)."
      echo "export KODACT_HARBOR_CA_BUNDLE=$BUNDLE"
    fi
  } >"$new"
  if [ -f "$CERTS_ENV" ] && cmp -s "$new" "$CERTS_ENV"; then
    ok "up to date"
  elif $CHECK; then
    todo "$CERTS_ENV is missing or out of date"
  elif ! $REACHED && [ -f "$CERTS_ENV" ]; then
    note "no site answered, so $CERTS_ENV is left as it is"
  else
    cp "$new" "$CERTS_ENV"
    ok "written"
  fi
  if grep -qF '.kodact-certs.sh' "$BASHRC" 2>/dev/null; then
    ok "$BASHRC reads it"
  elif $CHECK; then
    todo "$BASHRC doesn't read $CERTS_ENV"
  else
    # shellcheck disable=SC2016 # $HOME is for ~/.bashrc to expand
    printf '\n%s\n' '[ -f "$HOME/.kodact-certs.sh" ] && . "$HOME/.kodact-certs.sh"  # KodAct: certificates (scripts/setup-debian.sh)' >>"$BASHRC"
    ok "$BASHRC reads it from now on"
  fi
  # shellcheck disable=SC1090
  if [ -f "$CERTS_ENV" ]; then . "$CERTS_ENV"; fi
}

# --- Packages and tools ---------------------------------------------------------------------

PACKAGES_OK=true
step_packages() {
  step "Packages"
  local missing=() p st
  for p in "${PACKAGES[@]}"; do
    st="$(dpkg-query -W -f='${Status}' "$p" 2>/dev/null || true)"
    [ "$st" = "install ok installed" ] || missing+=("$p")
  done
  if [ ${#missing[@]} -eq 0 ]; then
    ok "all installed: ${PACKAGES[*]}"
    PACKAGES_OK=true
    return 0
  fi
  if $CHECK; then
    todo "not installed: ${missing[*]}"
    return 0
  fi
  if run_logged $SUDO apt-get update -qq &&
    run_logged $SUDO env DEBIAN_FRONTEND=noninteractive apt-get install -y -qq --no-install-recommends "${missing[@]}"; then
    ok "installed ${missing[*]}"
    PACKAGES_OK=true
  else
    PACKAGES_OK=false
    fail "apt-get couldn't install ${missing[*]} (see above)"
  fi
}

# True when version $1 is at least $2.
version_ge() { [ "$(printf '%s\n%s\n' "$2" "$1" | sort -V | head -n 1)" = "$2" ]; }

step_rust() {
  step "Rust"
  # shellcheck disable=SC1091
  if [ -f "$HOME/.cargo/env" ]; then . "$HOME/.cargo/env"; fi
  local need have=""
  need="$(sed -n 's/^rust-version *= *"\(.*\)"/\1/p' "$REPO/Cargo.toml" 2>/dev/null | head -n 1)"
  need="${need:-1.87}"
  if command -v rustc >/dev/null 2>&1; then have="$(rustc --version | awk '{ print $2 }')"; fi
  if ! command -v rustup >/dev/null 2>&1 && { [ -z "$have" ] || ! version_ge "$have" "$need"; }; then
    if $CHECK; then
      todo "Rust $need or later is needed (${have:-none installed}): rustup installs it"
      return 0
    fi
    # Debian's own rustc, if any, stays: ~/.cargo/bin comes first on PATH.
    if ! run_logged env RUSTUP_INIT_SKIP_PATH_CHECK=yes bash -o pipefail -c \
      "curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --component rustfmt,clippy"; then
      fail "rustup couldn't install Rust (see above)"
      return 0
    fi
    # shellcheck disable=SC1091
    . "$HOME/.cargo/env"
  elif command -v rustup >/dev/null 2>&1 && ! $CHECK; then
    if ! version_ge "$have" "$need"; then run_logged rustup update stable || true; fi
    run_logged rustup component add rustfmt clippy || true
  fi
  have="$(rustc --version | awk '{ print $2 }')"
  if version_ge "$have" "$need"; then ok "$(rustc --version)"; else fail "rustc $have is older than the $need this workspace needs: rustup update stable"; fi
  cargo fmt --version >/dev/null 2>&1 || todo "rustfmt is missing: rustup component add rustfmt"
  cargo clippy --version >/dev/null 2>&1 || todo "clippy is missing: rustup component add clippy"
}

step_harbor() {
  step "uv and Harbor $HARBOR_VERSION"
  export PATH="$HOME/.local/bin:$PATH"
  if ! command -v uv >/dev/null 2>&1; then
    if $CHECK; then
      todo "uv is not installed"
      return 0
    fi
    if ! run_logged bash -o pipefail -c 'curl -LsSf https://astral.sh/uv/install.sh | sh'; then
      fail "the uv installer failed (see above)"
      return 0
    fi
  fi
  ok "$(uv --version)"
  local tools
  tools="$(uv tool list 2>/dev/null || true)"
  if grep -qx "harbor v$HARBOR_VERSION" <<<"$tools"; then
    ok "harbor $HARBOR_VERSION"
  elif $CHECK; then
    todo "harbor $HARBOR_VERSION is not installed"
  elif run_logged uv tool install --force --python 3.12 "harbor==$HARBOR_VERSION"; then
    ok "installed harbor $HARBOR_VERSION"
  else
    fail "uv tool install harbor==$HARBOR_VERSION failed (see above)"
  fi
}

# --- Docker ---------------------------------------------------------------------------------

install_docker() {
  local id codename arch
  id="$(. /etc/os-release && echo "$ID")"
  codename="$(. /etc/os-release && echo "${VERSION_CODENAME:-}")"
  case "$id" in
    debian | ubuntu) ;;
    *)
      fail "install Docker Engine by hand for this system: https://docs.docker.com/engine/install/"
      return 1
      ;;
  esac
  arch="$(dpkg --print-architecture)"
  $SUDO install -m 0755 -d /etc/apt/keyrings
  if ! run_logged $SUDO curl -fsSL "https://download.docker.com/linux/$id/gpg" -o /etc/apt/keyrings/docker.asc; then
    fail "can't download Docker's signing key (see above)"
    return 1
  fi
  $SUDO chmod a+r /etc/apt/keyrings/docker.asc
  echo "deb [arch=$arch signed-by=/etc/apt/keyrings/docker.asc] https://download.docker.com/linux/$id $codename stable" |
    $SUDO tee /etc/apt/sources.list.d/docker.list >/dev/null
  if ! run_logged $SUDO apt-get update -qq ||
    ! run_logged $SUDO env DEBIAN_FRONTEND=noninteractive apt-get install -y -qq docker-ce docker-ce-cli containerd.io docker-buildx-plugin; then
    fail "apt-get couldn't install Docker Engine (see above)"
    return 1
  fi
  $SUDO usermod -aG docker "${USER:-$(id -un)}"
  ok "installed Docker Engine"
}

# Puts systemd=true under [boot] in $WSL_CONF, keeping everything else in it.
set_wsl_systemd() {
  $SUDO python3 - "$WSL_CONF" <<'EOF'
import re, sys
path = sys.argv[1]
try:
    lines = open(path).read().splitlines()
except FileNotFoundError:
    lines = []
out, in_boot, done = [], False, False
for line in lines:
    s = line.strip()
    if s.startswith("["):
        if in_boot and not done:
            out.append("systemd=true")
            done = True
        in_boot = s.lower() == "[boot]"
    elif in_boot and re.match(r"systemd\s*=", s, re.I):
        if not done:
            out.append("systemd=true")
            done = True
        continue
    out.append(line)
if in_boot and not done:
    out.append("systemd=true")
    done = True
if not done:
    out += ([""] if out else []) + ["[boot]", "systemd=true"]
open(path, "w").write("\n".join(out) + "\n")
EOF
}

# Sets the "dns" key of $DAEMON_JSON to the servers given, keeping every other key.
set_docker_dns() {
  $SUDO python3 - "$DAEMON_JSON" "$@" <<'EOF'
import json, os, sys
path, servers = sys.argv[1], sys.argv[2:]
try:
    with open(path) as f:
        conf = json.load(f)
except FileNotFoundError:
    conf = {}
except json.JSONDecodeError as e:
    sys.exit(f"{path} is not valid JSON ({e}); fix it, then rerun")
conf["dns"] = servers
os.makedirs(os.path.dirname(path), exist_ok=True)
with open(path + ".tmp", "w") as f:
    f.write(json.dumps(conf, indent=2) + "\n")
os.replace(path + ".tmp", path)
EOF
}

restart_docker() {
  if has_systemd; then
    $SUDO systemctl restart docker
  else
    $SUDO service docker restart >/dev/null
  fi
  local _
  for _ in $(seq 20); do
    "${DOCKER_CMD[@]}" info >/dev/null 2>&1 && return 0
    sleep 1
  done
  return 1
}

# The daemon answers; under WSL without systemd it is started for this session.
docker_up() {
  if docker info >/dev/null 2>&1; then return 0; fi
  local err groups
  err="$(docker info 2>&1 >/dev/null || true)"
  if grep -qi 'permission denied' <<<"$err"; then
    local me="${USER:-$(id -un)}"
    groups="$(id -nG "$me" 2>/dev/null || true)"
    if ! grep -qw docker <<<"$groups" && ! $CHECK; then $SUDO usermod -aG docker "$me"; fi
    if [ -n "$SUDO" ] && ! $CHECK && sudo docker info >/dev/null 2>&1; then
      DOCKER_CMD=(sudo docker)
      todo "close and reopen the terminal, so that docker works without sudo (you're in the docker group now)"
      return 0
    fi
    todo "close and reopen the terminal (docker group), then rerun this script"
    return 1
  fi
  if $CHECK; then
    todo "the Docker daemon isn't running"
    return 1
  fi
  if has_systemd; then
    $SUDO systemctl enable --now docker >/dev/null 2>&1 || true
  else
    $SUDO service docker start >/dev/null 2>&1 || true
  fi
  local _
  for _ in $(seq 20); do
    if docker info >/dev/null 2>&1; then return 0; fi
    if [ -n "$SUDO" ] && sudo docker info >/dev/null 2>&1; then
      DOCKER_CMD=(sudo docker)
      todo "close and reopen the terminal, so that docker works without sudo (you're in the docker group now)"
      return 0
    fi
    sleep 1
  done
  fail "the Docker daemon doesn't start: see sudo journalctl -u docker (or /var/log/docker.log)"
  return 1
}

# The names (of those given after $1) that a container can't resolve; $1 is a DNS server for
# that container to use instead of the daemon's, or empty.
unresolved() {
  local dns=() n out bad=()
  if [ -n "$1" ]; then dns=(--dns "$1"); fi
  shift
  for n in "$@"; do
    out="$("${DOCKER_CMD[@]}" run --rm ${dns[@]+"${dns[@]}"} "$TEST_IMAGE" nslookup "$n" 2>/dev/null || true)"
    grep -q '^Name:' <<<"$out" || bad+=("$n")
  done
  echo "${bad[*]}"
}

# Docker Desktop keeps its daemon settings in its own window, not in /etc/docker/daemon.json.
docker_desktop() {
  local os
  os="$("${DOCKER_CMD[@]}" info --format '{{.OperatingSystem}}' 2>/dev/null || true)"
  grep -qi 'docker desktop' <<<"$os"
}

# DNS servers to try for containers: yours, this system's, and Windows' (the company's, on WSL).
dns_candidates() {
  {
    # shellcheck disable=SC2086 # one server per word
    printf '%s\n' ${KODACT_DOCKER_DNS:-}
    awk '$1 == "nameserver" { print $2 }' "$RESOLV_CONF" 2>/dev/null
    # shellcheck disable=SC2016 # PowerShell's own $_
    winps 'Get-DnsClientServerAddress -AddressFamily IPv4 | ForEach-Object { $_.ServerAddresses }' || true
  } | awk 'NF && !/^127\./ && !seen[$0]++'
}

step_docker() {
  step "Docker"
  if ! command -v docker >/dev/null 2>&1; then
    if $CHECK; then
      todo "Docker is not installed"
      return 0
    fi
    install_docker || return 0
  fi
  if is_wsl && ! has_systemd; then
    if $CHECK; then
      todo "WSL runs without systemd, so Docker doesn't start by itself"
    else
      set_wsl_systemd
      todo "systemd is on in $WSL_CONF from the next start: run 'wsl --shutdown' in Windows PowerShell, reopen this terminal, then rerun this script"
    fi
  fi
  docker_up || return 0
  ok "docker $("${DOCKER_CMD[@]}" version --format '{{.Server.Version}}' 2>/dev/null)"
  local desktop=false
  if docker_desktop; then desktop=true; fi
  # The daemon reads the trust store when it starts (Docker Desktop reads Windows' own).
  if $CA_CHANGED && ! $desktop; then restart_docker || true; fi
  if ! run_logged "${DOCKER_CMD[@]}" pull -q "$TEST_IMAGE"; then
    fail "docker can't pull $TEST_IMAGE (see above)"
    return 0
  fi
  local names=("${CONTAINER_NAMES[@]}") bad servers=() working=() s before="$WORK/daemon.json.before" had=false
  bad="$(unresolved "" "${names[@]}")"
  if [ -z "$bad" ]; then
    ok "containers resolve ${names[*]}"
  elif $CHECK; then
    fail "containers can't resolve $bad"
  else
    # Each candidate is tried from a container (docker run --dns) before anything is written.
    mapfile -t servers < <(dns_candidates)
    for s in ${servers[@]+"${servers[@]}"}; do
      if [ -z "$(unresolved "$s" "${names[@]}")" ]; then working+=("$s"); fi
    done
    if [ ${#working[@]} -eq 0 ]; then
      fail "containers can't resolve $bad, through the daemon or any of these DNS servers: ${servers[*]:-none}"
      note "see evals/harbor/README.md, \"Containers can't resolve names\"; then rerun with KODACT_DOCKER_DNS"
    elif $desktop; then
      local list
      list="$(printf '"%s", ' "${working[@]}")"
      fail "containers can't resolve $bad: in Docker Desktop, Settings > Docker Engine, set \"dns\": [${list%, }], then Apply"
    else
      if [ -f "$DAEMON_JSON" ]; then
        $SUDO cp "$DAEMON_JSON" "$before"
        had=true
      fi
      if ! set_docker_dns "${working[@]}"; then
        fail "$DAEMON_JSON is not valid JSON, so it was left as it is: fix it, then rerun"
      else
        restart_docker || true
        bad="$(unresolved "" "${names[@]}")"
        if [ -z "$bad" ]; then
          ok "containers resolve ${names[*]} through ${working[*]} (\"dns\" in $DAEMON_JSON)"
        else
          # It didn't help: put the file back as it was.
          if $had; then $SUDO cp "$before" "$DAEMON_JSON"; else $SUDO rm -f "$DAEMON_JSON"; fi
          restart_docker || true
          fail "containers still can't resolve $bad with \"dns\": [${working[*]}], so $DAEMON_JSON is back as it was"
        fi
      fi
    fi
  fi
  # A task container that trusts this machine's CAs, as Harbor's do with KODACT_HARBOR_CA_BUNDLE.
  if run_logged "${DOCKER_CMD[@]}" run --rm -v "$BUNDLE:/host-ca.pem:ro" "$CURL_IMAGE" \
    -fsS --cacert /host-ca.pem -o /dev/null https://pypi.org/simple/pip/; then
    ok "containers reach https://pypi.org with this machine's CAs"
  else
    fail "a container can't fetch https://pypi.org even with this machine's CAs (see above)"
  fi
}

# --- Checks ---------------------------------------------------------------------------------

check() {
  local what="$1"
  shift
  if run_logged "$@"; then ok "$what"; else fail "$what"; fi
}

step_checks() {
  step "Checks"
  check "curl: https://pypi.org" curl -fsS -o /dev/null https://pypi.org/simple/pip/
  check "git: https://github.com" git ls-remote https://github.com/git/git.git HEAD
  check "python: https://pypi.org" python3 -c 'import urllib.request; urllib.request.urlopen("https://pypi.org/simple/pip/", timeout=30)'
  check "pip: downloads from PyPI" python3 -m pip download -q --no-deps -d "$WORK/pip" six
  check "pytest" python3 -m pytest --version
  if command -v cargo >/dev/null 2>&1; then check "cargo: https://crates.io" cargo search --limit 1 serde; fi
  if command -v uv >/dev/null 2>&1; then
    # Into a file of its own: uv writes its output by renaming a file onto it, which as root would
    # replace /dev/null itself.
    # shellcheck disable=SC2016 # $1 is for the inner shell
    check "uv: resolves from PyPI" bash -o pipefail -c 'echo six | uv pip compile - -q --no-cache -o "$1"' _ "$WORK/uv-compile.txt"
  fi
  if bwrap --ro-bind / / --dev /dev --unshare-net true >/dev/null 2>&1; then
    ok "bubblewrap: the sandbox tests run"
  else
    note "bubblewrap can't create a sandbox here: cargo test skips the sandbox tests"
  fi
  local url="${KODACT_OPENAI_BASE_URL:-${KODACT_BASE_URL:-}}" code
  if [ -n "$url" ]; then
    code="$(curl -sS -o /dev/null -w '%{http_code}' --max-time 30 "${url%/}/models" 2>"$WORK/endpoint.err" || true)"
    if [ "${code:-000}" != 000 ]; then
      ok "endpoint: $url answers (HTTP $code)"
    else
      fail "endpoint: $url doesn't answer: $(tail -n 1 "$WORK/endpoint.err")"
    fi
  else
    note "no KODACT_OPENAI_BASE_URL or KODACT_BASE_URL set: source your env file and rerun to check the endpoint"
  fi
}

summary() {
  step "Summary"
  if [ ${#failures[@]} -eq 0 ] && [ ${#todos[@]} -eq 0 ]; then
    echo "   everything is in place"
  fi
  local x
  for x in ${failures[@]+"${failures[@]}"}; do echo "   FAIL  $x"; done
  for x in ${todos[@]+"${todos[@]}"}; do echo "   TODO  $x"; done
  cat <<EOF

   Open a new terminal (or run: source $CERTS_ENV), then:
     cargo build && cargo test --workspace
     evals/harbor/build-static.sh
     source ~/.kodact-env && evals/harbor/run.sh smoke
EOF
  [ ${#failures[@]} -eq 0 ]
}

main() {
  local a
  for a in "$@"; do
    case "$a" in
      --check) CHECK=true ;;
      --no-docker) DOCKER=false ;;
      -h | --help)
        awk 'NR > 1 && /^#/ { sub(/^# ?/, ""); print; next } NR > 1 { exit }' "$0"
        return 0
        ;;
      *)
        echo "setup-debian: unknown option $a (try --help)" >&2
        return 2
        ;;
    esac
  done
  if [ ! -r /etc/debian_version ]; then
    echo "setup-debian: this script is for Debian and Ubuntu" >&2
    return 1
  fi
  WORK="$(mktemp -d)"
  trap 'rm -rf "$WORK"' EXIT
  if [ -n "$SUDO" ] && ! $CHECK; then sudo -v; fi # the password once, up front
  step_packages
  step_ca
  if ! $PACKAGES_OK && $CA_CHANGED; then step_packages; fi # apt sources over HTTPS need the CA
  step_certs_env
  step_rust
  step_harbor
  if $DOCKER; then step_docker; fi
  step_checks
  summary
}

if [ "${BASH_SOURCE[0]}" = "$0" ]; then main "$@"; fi
