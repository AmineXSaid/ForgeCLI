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

The model comes from `harbor run -m provider/name`; the provider part is
dropped, so `-m openai/deep-thinking` runs `forge --model deep-thinking`.

Run it through `evals/harbor/run.sh`. Written against Harbor 0.24.0.
"""

import json
import os
import shlex
from pathlib import Path

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

    @with_prompt_template
    async def run(self, instruction: str, environment: BaseEnvironment, context: AgentContext) -> None:
        model = (self.model_name or "").split("/", 1)[-1]
        env = {
            k: v
            for k, v in os.environ.items()
            if k.startswith("FORGE_") and k != "FORGE_STATIC_BIN" and not k.startswith("FORGE_HARBOR_")
        }
        env["FORGE_TASK"] = instruction
        flags = ["--dangerously-skip-permissions", "--output-format", "stream-json", "--verbose"]
        if model:
            flags += ["--model", model]
        flags += shlex.split(os.environ.get("FORGE_HARBOR_ARGS", ""))
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
