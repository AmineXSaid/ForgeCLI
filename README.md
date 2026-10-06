# ForgeCLI

ForgeCLI is an agentic coding CLI written in Rust: an agent loop, a tool
system, permission modes, MCP, context compaction, hooks, a stream-json
protocol for IDE hosts, and a terminal UI.

ForgeCLI is written from public interface documentation only, and it ships
its own system prompt and tool descriptions.

## Goal

`docs/GOALS.md` defines what ForgeCLI optimizes: the best measured task success
per model, cost and time. Every feature is built against it and measured with
`forge-eval` (see `FORGE.md`).

## Status

Under construction. `docs/PARITY.md` tracks every tool, flag, protocol message
and command against the reference CLI, with the test that proves each one. `docs/ARCHITECTURE.md` holds
the crate layout and the behavioural contracts (C1–C11).

## Build

```bash
cargo build --release
./target/release/forge --help
```

## Credentials

| Variable | Used for |
| --- | --- |
| `FORGE_API_KEY` | API key, sent as `x-api-key` |
| `FORGE_AUTH_TOKEN` | Bearer token, for gateways and proxies |
| `FORGE_BASE_URL` | A different Messages API endpoint |
| `FORGE_CUSTOM_HEADERS` | Extra request headers, one `Name: value` per line |
| `FORGE_OPENAI_BASE_URL`, `FORGE_OPENAI_API_KEY` | An OpenAI-compatible endpoint |

## Optional external prompt set

Point `FORGE_PROMPTS_DIR` at a directory containing:
- `system/*.md`, which replace the built-in system prompt sections;
- `tools/<ToolName>.md`, which replace individual tool descriptions.

Keep such a directory out of this repository.
