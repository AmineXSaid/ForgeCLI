"""ForgeCLI as a Harbor installed agent, for Terminal-Bench runs.

Harbor starts each task's container, calls `install()`, then `run()` with the
task instruction, and grades the container afterwards. This agent copies a
static `forge` binary into the container and runs it once in print mode.

Configuration comes from the environment of the `harbor` process:

- `FORGE_STATIC_BIN`: the static binary to copy (default:
  `target-static/release/forge`, built by `evals/harbor/build-static.sh`).
- Every `FORGE_*` variable (endpoint, key, context window, ...) is passed to
  `forge` inside the container. The key is never put on the command line.
- `FORGE_HARBOR_ARGS`: extra `forge` flags for every task (for example
  `--max-turns 100`).
- `FORGE_HARBOR_AUTONOMOUS`: `forge` runs with `--autonomous` (no question tools,
  attempt before giving up); `0` turns it off, for A/B runs.
- `FORGE_HARBOR_TIME_LIMIT`: seconds for `--max-time`. By default the task's own
  agent time limit is used (read from the running trial), minus a margin, so
  the model knows its deadline and is warned before it.
- `FORGE_HARBOR_HOSTS`: extra host names (comma-separated) to resolve on this
  machine and pin in each container's `/etc/hosts`. The endpoint's host
  (`FORGE_OPENAI_BASE_URL` or `FORGE_BASE_URL`) is always pinned, so an
  endpoint behind company DNS works from task containers;
  `FORGE_HARBOR_HOSTS=none` turns pinning off.
- `FORGE_HARBOR_HOST_IP`: a self-hosted endpoint on this machine (`localhost`,
  `127.0.0.1`) means the container itself inside a task container, so such a URL
  is rewritten to this machine's address, found automatically or set here. The
  server must listen on that address (`0.0.0.0`), not only on `127.0.0.1`.
- `FORGE_HARBOR_CA_BUNDLE`: a PEM file of extra certificate authorities to
  trust inside the task container (behind a TLS-inspecting proxy). This
  changes the task environment: scores are for local comparisons only.

The model comes from `harbor run -m provider/name`; the provider part is
dropped, so `-m openai/deep-thinking` and `-m deep-thinking` both run `forge --model deep-thinking`.

Run it through `evals/harbor/run.sh`. Written against Harbor 0.24.0.
"""

import inspect
import json
import os
import shlex
import socket
from pathlib import Path
from urllib.parse import urlparse

from harbor.agents.installed.base import BaseInstalledAgent, with_prompt_template
from harbor.environments.base import BaseEnvironment
from harbor.models.agent.context import AgentContext

REPO = Path(__file__).resolve().parents[2]
DEFAULT_BIN = REPO / "target-static" / "release" / "forge"
REMOTE_BIN = "/usr/local/bin/forge"
# Inside the container; Harbor collects /logs/agent into this trial's logs_dir.
LOG_DIR = "/logs/agent"
STREAM_LOG = "forge.jsonl"
STDERR_LOG = "forge.stderr.txt"
# Seconds kept back from the task's limit, so the final report is written before the cut-off.
TIME_MARGIN = 30
REMOTE_CA = "/tmp/forge-extra-ca.pem"
# Trust REMOTE_CA in the container: in the system store (now, and for any later
# `update-ca-certificates`, e.g. when a test script installs curl), in existing bundle files,
# and for uv, which test scripts use to install their tools.
TRUST_CA = r"""
src=%s
mkdir -p /usr/local/share/ca-certificates
if command -v awk >/dev/null 2>&1; then
  awk '/-----BEGIN CERTIFICATE-----/{n++; f=sprintf("/usr/local/share/ca-certificates/forge-extra-%%03d.crt", n)} n>0{print > f}' "$src"
else
  cp "$src" /usr/local/share/ca-certificates/forge-extra.crt
fi
if command -v update-ca-certificates >/dev/null 2>&1; then update-ca-certificates >/dev/null 2>&1 || true; fi
if [ -d /etc/pki/ca-trust/source/anchors ]; then
  cp "$src" /etc/pki/ca-trust/source/anchors/forge-extra.pem
  command -v update-ca-trust >/dev/null 2>&1 && update-ca-trust >/dev/null 2>&1 || true
fi
for b in /etc/ssl/certs/ca-certificates.crt /etc/pki/tls/certs/ca-bundle.crt /etc/ssl/cert.pem; do
  if [ -f "$b" ]; then cat "$src" >> "$b"; fi
done
mkdir -p /etc/uv && printf 'native-tls = true\n' > /etc/uv/uv.toml
""" % REMOTE_CA


def pinned_hosts() -> list[tuple[str, str]]:
    """(ip, name) for the endpoint's host and FORGE_HARBOR_HOSTS, as this machine resolves them.

    Task containers use Docker's DNS, which doesn't know names that only company DNS
    resolves; pinning them in /etc/hosts lets forge reach such an endpoint.
    """
    extra = os.environ.get("FORGE_HARBOR_HOSTS", "")
    if extra.strip().lower() == "none":
        return []
    names = [n.strip() for n in extra.split(",") if n.strip()]
    for var in ("FORGE_OPENAI_BASE_URL", "FORGE_BASE_URL"):
        host = urlparse(os.environ.get(var, "")).hostname
        if host:
            names.append(host)
    out = []
    for name in dict.fromkeys(names):
        if name.lower() in LOOPBACK:
            continue  # rewritten to this machine's address by container_url()
        try:
            socket.inet_aton(name)
            continue  # already an address
        except OSError:
            pass
        try:
            out.append((socket.gethostbyname(name), name))
        except OSError:
            pass  # unresolvable here too: leave it to the container's DNS
    return out


LOOPBACK = ("localhost", "127.0.0.1", "0.0.0.0", "::1")


def host_ip() -> str:
    """This machine's address as task containers can reach it (FORGE_HARBOR_HOST_IP overrides)."""
    ip = os.environ.get("FORGE_HARBOR_HOST_IP")
    if ip:
        return ip
    try:
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as s:
            s.connect(("10.255.255.255", 1))  # picks the outgoing interface; sends nothing
            return s.getsockname()[0]
    except OSError as e:
        raise RuntimeError(
            f"cannot find this machine's address for a localhost endpoint ({e}); "
            "set FORGE_HARBOR_HOST_IP to an address task containers can reach"
        ) from e


def container_url(url: str) -> str:
    """`url` as seen from a task container: a loopback host becomes this machine's address."""
    parts = urlparse(url)
    if (parts.hostname or "").lower() not in LOOPBACK and not (parts.hostname or "").startswith("127."):
        return url
    ip = host_ip()
    userinfo = parts.netloc.rpartition("@")[0]
    netloc = (f"{userinfo}@" if userinfo else "") + (f"[{ip}]" if ":" in ip else ip)
    netloc += f":{parts.port}" if parts.port else ""
    return parts._replace(netloc=netloc).geturl()


def task_time_limit() -> float | None:
    """The agent time limit of the trial running this agent, in seconds.

    Harbor gives it only to its oracle agent, so it is read from the Trial on the
    call stack (Harbor 0.24 awaits `run()` directly); FORGE_HARBOR_TIME_LIMIT overrides it.
    """
    override = os.environ.get("FORGE_HARBOR_TIME_LIMIT")
    if override:
        return float(override)
    frame = inspect.currentframe()
    while frame is not None:
        value = getattr(frame.f_locals.get("self"), "_agent_timeout_sec", None)
        if isinstance(value, (int, float)) and value > 0:
            return float(value)
        frame = frame.f_back
    return None


class ForgeCLI(BaseInstalledAgent):
    @staticmethod
    def name() -> str:
        return "forgecli"

    def get_version_command(self) -> str | None:
        return f"{REMOTE_BIN} --version"

    async def install(self, environment: BaseEnvironment) -> None:
        binary = Path(os.environ.get("FORGE_STATIC_BIN", DEFAULT_BIN))
        if not binary.is_file():
            raise RuntimeError(f"{binary} not found: run evals/harbor/build-static.sh first")
        await environment.upload_file(binary, REMOTE_BIN)
        await self.exec_as_root(environment, command=f"chmod 755 {REMOTE_BIN} && {REMOTE_BIN} --version")
        hosts = pinned_hosts()
        if hosts:
            lines = "\n".join(f"{ip} {name}" for ip, name in hosts)
            await self.exec_as_root(environment, command=f"printf '%s\\n' {shlex.quote(lines)} >> /etc/hosts")
        ca = os.environ.get("FORGE_HARBOR_CA_BUNDLE")
        if ca:
            if not Path(ca).is_file():
                raise RuntimeError(f"FORGE_HARBOR_CA_BUNDLE: {ca} not found")
            await environment.upload_file(Path(ca), REMOTE_CA)
            await self.exec_as_root(environment, command=TRUST_CA)

    @with_prompt_template
    async def run(self, instruction: str, environment: BaseEnvironment, context: AgentContext) -> None:
        model = (self.model_name or "").split("/", 1)[-1]
        env = {
            k: v
            for k, v in os.environ.items()
            if k.startswith("FORGE_") and k != "FORGE_STATIC_BIN" and not k.startswith("FORGE_HARBOR_")
        }
        for var in ("FORGE_OPENAI_BASE_URL", "FORGE_BASE_URL"):
            if env.get(var):
                env[var] = container_url(env[var])
        env["FORGE_TASK"] = instruction
        flags = ["--dangerously-skip-permissions", "--output-format", "stream-json", "--verbose"]
        if os.environ.get("FORGE_HARBOR_AUTONOMOUS", "1").strip().lower() not in ("0", "false", "no", "off"):
            flags.append("--autonomous")
        if model:
            flags += ["--model", model]
        extra = shlex.split(os.environ.get("FORGE_HARBOR_ARGS", ""))
        limit = task_time_limit()
        if limit and "--max-time" not in extra:
            flags += ["--max-time", str(max(60, int(limit) - TIME_MARGIN))]
        flags += extra
        command = (
            f"mkdir -p {LOG_DIR}; "
            f'{REMOTE_BIN} -p "$FORGE_TASK" {shlex.join(flags)} '
            f"> {LOG_DIR}/{STREAM_LOG} 2> {LOG_DIR}/{STDERR_LOG}"
        )
        await self.exec_as_agent(environment, command=command, env=env)

    def populate_context_post_run(self, context: AgentContext) -> None:
        """Tokens and cost from the final `result` line of the stream."""
        path = self.logs_dir / STREAM_LOG
        if not path.is_file():
            return
        result = None
        for line in path.read_text(errors="replace").splitlines():
            try:
                event = json.loads(line)
            except ValueError:
                continue
            if isinstance(event, dict) and event.get("type") == "result":
                result = event
        if result is None:
            return
        usage = result.get("usage") or {}
        context.n_input_tokens = usage.get("input_tokens", 0) + usage.get("cache_read_input_tokens", 0)
        context.n_cache_tokens = usage.get("cache_read_input_tokens", 0)
        context.n_output_tokens = usage.get("output_tokens", 0)
        context.cost_usd = result.get("total_cost_usd")
        context.metadata = {
            "num_turns": result.get("num_turns"),
            "subtype": result.get("subtype"),
            "stop_reason": result.get("stop_reason"),
        }
