# Terminal-Bench with Harbor

`forge-eval` (in `evals/tasks/`) is the quick local suite. Terminal-Bench is
the external benchmark: 89 tasks, each in its own container, graded by hidden
tests. [Harbor](https://github.com/laude-institute/harbor) runs it. This
directory holds a ForgeCLI agent for Harbor (`forge_agent.py`) and two scripts.

How a task runs:
1. Harbor starts the task's container.
2. The agent copies a static `forge` into it (`/usr/local/bin/forge`).
3. The agent runs `forge -p "<task>" --dangerously-skip-permissions --output-format stream-json`
   once.
4. Harbor runs the hidden tests.

The stream ends up in each trial's agent logs (`forge.jsonl`,
`forge.stderr.txt`). Tokens and cost come from its final `result` line.

## Setup (once)

You need Docker, `uv` and Harbor:

```bash
docker run --rm hello-world
curl -LsSf https://astral.sh/uv/install.sh | sh && source "$HOME/.local/bin/env"
uv tool install --python 3.12 harbor     # Python 3.12: some 3.14 builds crash loading PyYAML
harbor --help
```

Build the static binary:

```bash
evals/harbor/build-static.sh             # Docker rust:alpine -> target-static/release/forge
```

Rebuild it after every code change you want to measure.

## Running

The `FORGE_*` variables of your shell (endpoint, key, `FORGE_CONTEXT_WINDOW`,
...) are passed into every container. The key is never put on a command line.

```bash
source ~/.forge-env
evals/harbor/run.sh subset               # first 10 tasks, 1 attempt: check that it works
evals/harbor/run.sh full                 # all tasks, 5 attempts: the leaderboard setup
evals/harbor/run.sh subset -i 'hello*'   # extra args go to `harbor run`
```

| Variable | Default | Meaning |
| --- | --- | --- |
| `FORGE_HARBOR_MODEL` | `openai/deep-thinking` | `provider/name`; `forge` gets `--model name` |
| `FORGE_HARBOR_DATASET` | `terminal-bench@2.0` | Harbor dataset |
| `FORGE_HARBOR_JOBS` | `2` | Tasks in parallel |
| `FORGE_HARBOR_ARGS` | | Extra `forge` flags, e.g. `--max-turns 100` |
| `FORGE_STATIC_BIN` | `target-static/release/forge` | Binary copied into containers |
| `FORGE_HARBOR_TIME_LIMIT` | the task's own limit | Seconds for `forge --max-time` |
| `FORGE_HARBOR_CA_BUNDLE` | | Extra CAs to trust inside containers (see below) |

Each task has an agent time limit, and Harbor stops the agent when it's up.
The agent passes that limit, less 30 seconds, as `--max-time`. The model then
knows its deadline from the start and gets a wrap-up reminder near the end.

The job name records the commit, so every result is pinned to a build. A
`-dirty` suffix means uncommitted changes to `crates/`.

## Comparing harnesses

To compare harnesses, keep the model and the tasks fixed and change only the
agent:
- `-a opencode` or another built-in Harbor agent, against this one;
- two ForgeCLI builds;
- `FORGE_VERIFY=0` against the default.

Ten tasks are enough to see a large difference. Use the full run before you
claim a small one.

## Things to check first

- **Network.** The containers must reach your endpoint. A gateway reachable only
  on a company network or VPN may not be reachable from Docker.
- **Rate limits and cost.** The full run is 89 × 5 trials. Run the subset first.
- **Keys.** The key is copied into every task container. Use a key made for
  benchmark runs.

`forge_agent.py` was written against Harbor 0.24.0 and checked against its
installed-agent API with a fake environment. It has not been run against real
task containers here.

## Behind a company proxy

If `harbor run` fails with `CERTIFICATE_VERIFY_FAILED`, your network
re-signs HTTPS traffic with its own certificate authority, and Harbor's Python
doesn't trust it. Point Python at the system's trust store, which should
already hold that authority:

```bash
export SSL_CERT_FILE=/etc/pki/tls/certs/ca-bundle.crt      # Fedora/CentOS/RHEL
# export SSL_CERT_FILE=/etc/ssl/certs/ca-certificates.crt  # Debian/Ubuntu
```

If that file doesn't have it either, ask IT for the root certificate and add
it to the system trust store (`/etc/pki/ca-trust/source/anchors/` then
`sudo update-ca-trust`, or `/usr/local/share/ca-certificates/` then
`sudo update-ca-certificates`).

The task containers have the same problem. Their test scripts download tools
(`uv`, packages) over HTTPS, so on such a network they fail and every task
scores 0, whatever the agent did. To work around it:

```bash
export FORGE_HARBOR_CA_BUNDLE=/etc/pki/tls/certs/ca-bundle.crt
evals/harbor/run.sh subset
```

The agent adds those certificates to each container's trust store, including
for any later `update-ca-certificates`, and turns on `native-tls` for `uv`.
This changes the task environment, so:
- these jobs are named `-hostca`;
- their scores only compare ForgeCLI builds with each other;
- they aren't comparable with the leaderboard, and must not be submitted.

Tools that bring their own certificate list (for example `pip` through
`certifi`) may still fail. For real scores, run from a network without TLS
inspection.
