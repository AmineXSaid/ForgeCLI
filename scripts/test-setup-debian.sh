#!/usr/bin/env bash
# Offline tests for scripts/setup-debian.sh: which CA it trusts, and that its edits to
# /etc/wsl.conf, /etc/docker/daemon.json and ~/.kodact-certs.sh keep what is already there.
# Needs bash, openssl and python3; no network, no root, nothing outside a temporary directory.
#
#   scripts/test-setup-debian.sh
set -euo pipefail
# shellcheck disable=SC2034 # the settings below are read by the functions sourced from setup-debian.sh
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source-path=SCRIPTDIR source=setup-debian.sh
source "$here/setup-debian.sh"
set +e # the checks below look at failures themselves

T="$(mktemp -d)"
trap 'rm -rf "$T"' EXIT
WORK="$T/work"
mkdir -p "$WORK" "$T/anchors" "$T/home"
SUDO=
ANCHORS="$T/anchors"
BUNDLE="$T/bundle.crt"
CERTS_ENV="$T/home/.kodact-certs.sh"
BASHRC="$T/home/.bashrc"
passed=0
failed=0
expect() { # description, then a command that must succeed
  local what="$1"
  shift
  if "$@"; then
    passed=$((passed + 1))
  else
    failed=$((failed + 1))
    echo "FAIL: $what"
  fi
}
refute() { # description, then a command that must fail
  local what="$1"
  shift
  if "$@"; then
    failed=$((failed + 1))
    echo "FAIL: $what"
  else
    passed=$((passed + 1))
  fi
}

# --- A company proxy's chain: root > intermediate > a leaf for github.com -------------------
cert() { # name, subject, issuer name (empty: self-signed), CA or leaf
  local ext="$T/$1.ext"
  if [ "$4" = ca ]; then
    printf 'basicConstraints=critical,CA:TRUE\nkeyUsage=critical,keyCertSign,cRLSign\n' >"$ext"
  else
    printf 'subjectAltName=DNS:github.com\n' >"$ext"
  fi
  openssl req -new -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -keyout "$T/$1.key" \
    -out "$T/$1.csr" -subj "$2" 2>/dev/null
  if [ -z "$3" ]; then
    openssl x509 -req -in "$T/$1.csr" -key "$T/$1.key" -out "$T/$1.pem" -days 30 -extfile "$ext" 2>/dev/null
  else
    openssl x509 -req -in "$T/$1.csr" -CA "$T/$3.pem" -CAkey "$T/$3.key" -CAcreateserial \
      -out "$T/$1.pem" -days 30 -extfile "$ext" 2>/dev/null
  fi
}
cert root "/O=Example Corp/CN=Example Corp Root CA" "" ca
cert inter "/O=Example Corp/CN=Example Corp Intermediate CA" root ca
cert leaf "/CN=github.com" inter leaf
# Same name as the company root, another key: it must never be taken for it.
cert decoy "/O=Example Corp/CN=Example Corp Root CA" "" ca
cert other "/O=Somewhere Else/CN=Unrelated Root" "" ca
cat "$T/leaf.pem" "$T/inter.pem" >"$T/sent.pem"                # what the proxy sends
cat "$T/leaf.pem" "$T/inter.pem" "$T/root.pem" >"$T/sent-root.pem" # a proxy that sends its root too
cat "$T/other.pem" >"$T/public.pem"                              # the public roots of a fresh system
# OpenSSL's default certificate directory holds the company root, as on a machine where it is
# installed already: checks against a given store must not look there too.
mkdir -p "$T/default-dir"
cp "$T/root.pem" "$T/default-dir/"
openssl rehash "$T/default-dir" 2>/dev/null
export SSL_CERT_DIR="$T/default-dir"

index_of() { # PEM file with candidates -> index file
  rm -rf "$T/cand"
  mkdir -p "$T/cand"
  split_pem "$1" "$T/cand" w
  index_candidates "$T/cand" >"$T/index"
}
same_cert() { [ "$(openssl x509 -in "$1" -noout -fingerprint -sha256)" = "$(openssl x509 -in "$2" -noout -fingerprint -sha256)" ]; }

refute "the proxy's chain isn't trusted by the public roots" chain_verifies "$T/public.pem" "$T/sent.pem"
expect "the chain is trusted with the company root" chain_verifies <(cat "$T/public.pem" "$T/root.pem") "$T/sent.pem"
refute "a root the site sends itself is not trusted for that" chain_verifies "$T/public.pem" "$T/sent-root.pem"

# Windows' store holds the decoy (same name, listed first), an unrelated root and the real one.
cat "$T/decoy.pem" "$T/other.pem" "$T/root.pem" >"$T/windows.pem"
index_of "$T/windows.pem"
mapfile -t found < <(signers_for "$T/sent.pem" "$T/index")
expect "one signer is found" [ "${#found[@]}" -eq 1 ]
expect "the signer is the company root, not the decoy with its name" same_cert "${found[0]:-/dev/null}" "$T/root.pem"
mapfile -t found < <(signers_for "$T/sent-root.pem" "$T/index")
expect "a site that sends its root: Windows' copy of it is the signer" same_cert "${found[0]:-/dev/null}" "$T/root.pem"

# A store without the company root finds nothing, even with the decoy in it.
cat "$T/decoy.pem" "$T/other.pem" >"$T/windows-without.pem"
index_of "$T/windows-without.pem"
mapfile -t found < <(signers_for "$T/sent.pem" "$T/index")
expect "no signer when the store lacks the company root" [ "${#found[@]}" -eq 0 ]

# Only the intermediate in the store: it is found, but with no root above it the chain
# still doesn't verify, so step_ca would not add it.
index_of "$T/inter.pem"
mapfile -t found < <(signers_for "$T/leaf.pem" "$T/index")
expect "an intermediate alone is found" [ "${#found[@]}" -eq 1 ]
refute "an intermediate alone doesn't make the chain trusted" chain_verifies <(cat "$T/public.pem" "${found[@]}") "$T/leaf.pem"

# --- step_ca end to end, with the site's answers and update-ca-certificates stood in --------
presented_chain() { cat "$T/site-$1.pem" 2>/dev/null || true; }
mkdir -p "$T/bin"
cat >"$T/bin/update-ca-certificates" <<EOF
#!/bin/sh
cat "$T/public.pem" "$T"/anchors/*.crt >"$BUNDLE" 2>/dev/null
EOF
chmod +x "$T/bin/update-ca-certificates"
PATH="$T/bin:$PATH"
MOZILLA="$T/mozilla"
mkdir -p "$MOZILLA"
cp "$T/other.pem" "$MOZILLA/other.crt"
cp "$T/public.pem" "$BUNDLE"
cp "$T/sent.pem" "$T/site-github.com.pem"
SITES=(github.com)
export KODACT_CA_FILE="$T/windows.pem"
CHECK=false
step_ca >"$T/out1" 2>&1
expect "step_ca sees the inspection" "$INTERCEPTED"
expect "step_ca adds exactly one CA" [ "$(find "$ANCHORS" -name 'kodact-company-ca-*.crt' | wc -l)" -eq 1 ]
expect "the CA it adds is the company root" same_cert "$(find "$ANCHORS" -name 'kodact-company-ca-*.crt' | head -n 1)" "$T/root.pem"
expect "github.com is trusted afterwards" grep -q 'github.com: trusted now' "$T/out1"
before="$(ls -l --time-style=+%s.%N "$ANCHORS")"
CA_CHANGED=false
INTERCEPTED=false
step_ca >"$T/out2" 2>&1
expect "a second run adds nothing" [ "$before" = "$(ls -l --time-style=+%s.%N "$ANCHORS")" ]
expect "a second run still sees the inspection" "$INTERCEPTED"
refute "a second run changes nothing" "$CA_CHANGED"

# A DER file (Windows' default export) works as KODACT_CA_FILE.
rm -f "$ANCHORS"/*.crt
cp "$T/public.pem" "$BUNDLE"
openssl x509 -in "$T/root.pem" -outform der -out "$T/root.cer"
KODACT_CA_FILE="$T/root.cer"
CA_CHANGED=false
step_ca >"$T/out3" 2>&1
expect "a DER KODACT_CA_FILE is read" same_cert "$(find "$ANCHORS" -name 'kodact-company-ca-*.crt' | head -n 1)" "$T/root.pem"

# Without a source (not WSL, no KODACT_CA_FILE) it adds nothing and says what to do.
rm -f "$ANCHORS"/*.crt
cp "$T/public.pem" "$BUNDLE"
unset KODACT_CA_FILE
is_wsl() { return 1; }
failures=()
step_ca >"$T/out4" 2>&1
expect "no source: nothing added" [ "$(find "$ANCHORS" -name '*.crt' | wc -l)" -eq 0 ]
expect "no source: the failure names the CA" grep -q 'github.com: signed by O = Example Corp, CN = Example Corp Root CA, not trusted here' "$T/out4"
expect "no source: it says to set KODACT_CA_FILE" grep -q 'KODACT_CA_FILE=' "$T/out4"

# --check changes nothing.
CHECK=true
KODACT_CA_FILE="$T/windows.pem"
step_ca >"$T/out5" 2>&1
expect "--check adds no CA" [ "$(find "$ANCHORS" -name '*.crt' | wc -l)" -eq 0 ]
CHECK=false

# --- ~/.kodact-certs.sh: lines only added, never changed --------------------------------------
INTERCEPTED=true
step_certs_env >/dev/null 2>&1
expect "the certificate settings name the bundle" grep -q "^export SSL_CERT_FILE=$BUNDLE\$" "$CERTS_ENV"
expect "inspected HTTPS: Harbor's containers get the CA" grep -q '^export KODACT_HARBOR_CA_BUNDLE=' "$CERTS_ENV"
expect "bashrc reads the settings" grep -q 'kodact-certs.sh' "$BASHRC"
INTERCEPTED=false # another network, or the VPN off: the next Harbor runs must not change
step_certs_env >/dev/null 2>&1
expect "the Harbor CA stays once set" grep -q '^export KODACT_HARBOR_CA_BUNDLE=' "$CERTS_ENV"
step_certs_env >/dev/null 2>&1
expect "bashrc reads them once" [ "$(grep -c 'kodact-certs.sh' "$BASHRC")" -eq 1 ]
# The owner's edits: a changed value, a line commented out, a line deleted, a line of their own.
sed -i 's|^export SSL_CERT_FILE=.*|export SSL_CERT_FILE=/opt/my-bundle.pem|; s|^export PIP_CERT=|# export PIP_CERT=|; /^export NODE_EXTRA_CA_CERTS=/d' "$CERTS_ENV"
echo 'export MY_OWN=1' >>"$CERTS_ENV"
INTERCEPTED=true
step_certs_env >/dev/null 2>&1
expect "a value the owner changed is kept" grep -qx 'export SSL_CERT_FILE=/opt/my-bundle.pem' "$CERTS_ENV"
expect "the changed value isn't added back beside it" [ "$(grep -c 'SSL_CERT_FILE=' "$CERTS_ENV")" -eq 1 ]
expect "a line the owner commented out stays off" [ "$(grep -c 'PIP_CERT=' "$CERTS_ENV")" -eq 1 ]
expect "the owner's own line is kept" grep -qx 'export MY_OWN=1' "$CERTS_ENV"
expect "a missing line is added back" grep -qx "export NODE_EXTRA_CA_CERTS=$BUNDLE" "$CERTS_ENV"
expect "the Harbor CA isn't added twice" [ "$(grep -c '^export KODACT_HARBOR_CA_BUNDLE=' "$CERTS_ENV")" -eq 1 ]

# --- Harbor: an installed one, of any version, is kept -------------------------------------------
mkdir -p "$T/uvbin"
cat >"$T/uvbin/uv" <<EOF
#!/bin/sh
case "\$*" in
  --version) echo "uv 0.0.0" ;;
  "tool list") cat "$T/uv-tools" 2>/dev/null ;;
  *) echo "\$*" >>"$T/uv-calls" ;;
esac
EOF
chmod +x "$T/uvbin/uv"
harbor_case() { # what uv tool list prints
  printf '%s' "$1" >"$T/uv-tools"
  : >"$T/uv-calls"
  HOME="$T/home" PATH="$T/uvbin:/usr/bin:/bin" step_harbor >"$T/harbor.out" 2>&1
}
harbor_case $'harbor v0.25.1\n- harbor\n'
expect "another Harbor version is kept" [ ! -s "$T/uv-calls" ]
expect "it says the version it kept" grep -q 'harbor 0.25.1, kept' "$T/harbor.out"
harbor_case ""
expect "no Harbor: it installs the version kodact_agent.py is written for" grep -qx "tool install --python 3.12 harbor==$HARBOR_VERSION" "$T/uv-calls"

# --- /etc/wsl.conf: systemd on, everything else kept -----------------------------------------
WSL_CONF="$T/wsl.conf"
wsl_case() { # description, content before, content expected
  printf '%s' "$2" >"$WSL_CONF"
  [ -n "$2" ] || rm -f "$WSL_CONF"
  set_wsl_systemd
  expect "wsl.conf: $1" [ "$(cat "$WSL_CONF")" = "$3" ]
}
wsl_case "no file" "" $'[boot]\nsystemd=true'
wsl_case "other sections kept" $'[network]\ngenerateResolvConf=false\n' $'[network]\ngenerateResolvConf=false\n\n[boot]\nsystemd=true'
wsl_case "a [boot] command kept" $'[boot]\ncommand=echo hi\n[user]\ndefault=me\n' $'[boot]\ncommand=echo hi\nsystemd=true\n[user]\ndefault=me'
wsl_case "the owner's systemd=false is kept" $'# mine\n[boot]\nsystemd = false\n' $'# mine\n[boot]\nsystemd = false'
wsl_case "already on: unchanged" $'[boot]\nsystemd=true\n' $'[boot]\nsystemd=true'
printf '[boot]\nsystemd = False\n' >"$WSL_CONF"
expect "wsl.conf: what it says about systemd is read" [ "$(wsl_systemd_setting)" = false ]
rm -f "$WSL_CONF"
expect "wsl.conf: no file says nothing" [ -z "$(wsl_systemd_setting)" ]

# --- /etc/docker/daemon.json: a "dns" key added, never replaced ---------------------------------
DAEMON_JSON="$T/docker/daemon.json"
set_docker_dns 10.1.1.1 10.2.2.2
expect "daemon.json: created" python3 -c "import json,sys; assert json.load(open('$DAEMON_JSON')) == {'dns': ['10.1.1.1', '10.2.2.2']}"
printf '{"log-driver": "local", "features": {"buildkit": true}}\n' >"$DAEMON_JSON"
set_docker_dns 10.1.1.1
expect "daemon.json: dns added, other keys kept" python3 -c "import json; d = json.load(open('$DAEMON_JSON')); assert d == {'log-driver': 'local', 'dns': ['10.1.1.1'], 'features': {'buildkit': True}}, d"
printf '{"log-driver": "local", "dns": ["10.21.2.10"]}\n' >"$DAEMON_JSON"
cp "$DAEMON_JSON" "$T/owner.json"
refute "daemon.json: an existing dns key is refused" set_docker_dns 8.8.8.8
expect "daemon.json: the owner's dns servers stay" cmp -s "$DAEMON_JSON" "$T/owner.json"
expect "daemon.json: the existing dns is read" [ "$(daemon_dns)" = '["10.21.2.10"]' ]
printf '{"log-driver": "local",\n' >"$DAEMON_JSON"
cp "$DAEMON_JSON" "$T/broken.json"
set_docker_dns 10.1.1.1 >/dev/null 2>&1
expect "daemon.json: invalid JSON is left alone" cmp -s "$DAEMON_JSON" "$T/broken.json"

# --- Small helpers ------------------------------------------------------------------------------
NO_PROXY="localhost,.corp.example,*.svc.local,pypi.org"
refute "NO_PROXY: a listed host goes direct" uses_proxy pypi.org
refute "NO_PROXY: a subdomain of .corp.example goes direct" uses_proxy git.corp.example
refute "NO_PROXY: *.svc.local matches" uses_proxy a.svc.local
expect "NO_PROXY: other hosts use the proxy" uses_proxy github.com
NO_PROXY="*"
refute "NO_PROXY=*: everything goes direct" uses_proxy github.com
unset NO_PROXY
expect "version_ge" version_ge 1.97.0 1.87
refute "version_ge: older" version_ge 1.63.0 1.87

echo "$passed passed, $failed failed"
[ "$failed" -eq 0 ]
