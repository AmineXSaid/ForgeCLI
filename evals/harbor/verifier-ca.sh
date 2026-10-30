# Sourced by run.sh, run-pi.sh, run-ante.sh and run-forgecode.sh.
#
# With FORGE_HARBOR_CA_BUNDLE, the agent uploads the bundle, adds it to the container's trust store
# and keeps a copy at /usr/local/share/forge/ca-bundle.pem (KEPT_CA in forge_agent.py, outside
# /tmp, which a task or the agent may clear). Harbor then runs the task's tests in that same container, but
# the tools the test scripts use to fetch packages and data bring their own certificate lists:
# uv (rustls), pip and requests (certifi), huggingface_hub. Behind a TLS-inspecting proxy they
# reject its certificate, and tasks such as pytorch-model-cli (Torch from download.pytorch.org)
# and reshard-c4-data (the C4 dataset from huggingface.co) score 0 before any test runs.
# These verifier variables (`harbor run --ve`) point each of those tools at the bundle.
verifier_ca=()
if [ -n "${FORGE_HARBOR_CA_BUNDLE:-}" ]; then
  for var in SSL_CERT_FILE REQUESTS_CA_BUNDLE CURL_CA_BUNDLE PIP_CERT GIT_SSL_CAINFO NODE_EXTRA_CA_CERTS; do
    verifier_ca+=(--ve "$var=/usr/local/share/forge/ca-bundle.pem")
  done
  verifier_ca+=(--ve UV_NATIVE_TLS=1)
fi
