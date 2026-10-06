# ForgeCLI

ForgeCLI is an agentic coding CLI written in Rust. It reimplements the
behaviour of the Claude Code CLI (parity target **v2.1.290**): the agent loop,
the tool system, permission modes, MCP, context compaction, hooks, the
stream-json SDK protocol and a terminal UI.

It is a clean-room build:
- **Sources:** only public interfaces, namely the CLI's `--help` output, the
  documented Agent SDK stream-json protocol and the Anthropic Messages API docs.
  No Claude Code source code was used.
- **Prompts:** ForgeCLI ships its own system prompt and tool descriptions.
  Anthropic's prompt text is not included.
- **Affiliation:** ForgeCLI is not affiliated with or endorsed by Anthropic.

## Status

Under construction. `docs/PARITY.md` tracks every tool, flag, protocol message
and command, with the test that proves each one. `docs/ARCHITECTURE.md` holds
the crate layout and the behavioural contracts (C1–C11).

## Build

```bash
cargo build --release
./target/release/forge --help
```

## Credentials

| Variable | Used for |
| --- | --- |
| `ANTHROPIC_API_KEY` | API key, sent as `x-api-key` |
| `ANTHROPIC_AUTH_TOKEN` | Bearer token, for gateways and proxies |
| `ANTHROPIC_BASE_URL` | A different API endpoint |
| `FORGE_OPENAI_BASE_URL`, `FORGE_OPENAI_API_KEY` | An OpenAI-compatible endpoint |

## Optional external prompt set

Point `FORGE_PROMPTS_DIR` at a directory containing:
- `system/*.md`, which replace the built-in system prompt sections;
- `tools/<ToolName>.md`, which replace individual tool descriptions.

Keep such a directory out of this repository.
