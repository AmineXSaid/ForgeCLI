"""Forge Code (forgecode.dev) as a Harbor installed agent, next to ForgeCLI and Ante.

It copies a static Forge Code binary into the task container and runs it once
with `-p`, against the same OpenAI-compatible endpoint as ForgeCLI, so the three
harnesses can be compared on the same model and tasks.

Configuration comes from the environment of the `harbor` process:
- `FORGECODE_BIN`: the static binary to copy (default `~/.forgecode/forge`;
  `run-forgecode.sh` downloads `forge-x86_64-unknown-linux-musl` from the
  latest release when it is missing).
- `FORGE_OPENAI_BASE_URL`, `FORGE_OPENAI_API_KEY`: the endpoint and key, the
  same ones ForgeCLI uses. Forge Code gets them as `OPENAI_URL` and
  `OPENAI_API_KEY`.
- `FORGECODE_HARBOR_ARGS`: extra Forge Code flags for every task.
- `FORGE_HARBOR_HOSTS`, `FORGE_HARBOR_HOST_IP`, `FORGE_HARBOR_CA_BUNDLE`: as for
  ForgeCLI (see forge_agent.py).

Forge Code reads every `FORGE_*` variable as one of its own settings, so none of
ForgeCLI's are passed. Its provider and model are set with
`FORGE_SESSION__PROVIDER_ID` and `FORGE_SESSION__MODEL_ID`, its config lives in
a fresh directory per task, and its self-update is off. Its output goes to
`forgecode.stdout.txt` and `forgecode.stderr.txt`, which watch.py follows.
Written against Harbor 0.24.0.
"""

import os
import shlex
from pathlib import Path

from harbor.agents.installed.base import BaseInstalledAgent, with_prompt_template
from harbor.environments.base import BaseEnvironment
from harbor.models.agent.context import AgentContext

from forge_agent import REMOTE_CA, TRUST_CA, container_url, pinned_hosts, record_run, settings, task_time_limit

DEFAULT_BIN = Path.home() / ".forgecode" / "forge"
REMOTE_BIN = "/usr/local/bin/forgecode"
LOG_DIR = "/logs/agent"
CONFIG_DIR = "/tmp/forgecode-config"


class ForgeCode(BaseInstalledAgent):
    @staticmethod
    def name() -> str:
        return "forgecode"

    def get_version_command(self) -> str | None:
        return f"{REMOTE_BIN} --version"

    async def install(self, environment: BaseEnvironment) -> None:
        binary = Path(os.environ.get("FORGECODE_BIN", DEFAULT_BIN))
        if not binary.is_file():
            raise RuntimeError(f"{binary} not found: run evals/harbor/run-forgecode.sh, which downloads it")
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
        url = os.environ.get("FORGE_OPENAI_BASE_URL")
        key = os.environ.get("FORGE_OPENAI_API_KEY")
        if not url or not key:
            raise RuntimeError("FORGE_OPENAI_BASE_URL and FORGE_OPENAI_API_KEY must be set")
        env = {
            "OPENAI_URL": container_url(url).rstrip("/"),
            "OPENAI_API_KEY": key,
            "FORGE_CONFIG": CONFIG_DIR,
            "FORGE_SESSION__PROVIDER_ID": "openai_compatible",
            "FORGE_SESSION__MODEL_ID": model,
            "FORGE_UPDATES__AUTO_UPDATE": "false",
            "FORGECODE_TASK": instruction,
        }
        if os.environ.get("FORGE_HARBOR_CA_BUNDLE"):
            env["FORGE_HTTP__ROOT_CERT_PATHS"] = REMOTE_CA
        extra = os.environ.get("FORGECODE_HARBOR_ARGS", "")
        meta = {
            "harness": "forgecode",
            "model": model,
            "flags": ["-p", *shlex.split(extra)],
            "time_limit_s": task_time_limit(),
            "settings": settings(env),
        }
        command = (
            f"mkdir -p {LOG_DIR} {CONFIG_DIR}; "
            + record_run("FORGECODE_TASK", meta)
            + f'{REMOTE_BIN} -p "$FORGECODE_TASK" {extra} '
            f"> {LOG_DIR}/forgecode.stdout.txt 2> {LOG_DIR}/forgecode.stderr.txt"
        )
        await self.exec_as_agent(environment, command=command, env=env)

    def populate_context_post_run(self, context: AgentContext) -> None:
        pass
