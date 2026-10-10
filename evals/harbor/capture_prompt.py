#!/usr/bin/env python3
"""A fake OpenAI-compatible endpoint that saves what a coding agent sends it.

    python3 evals/harbor/capture_prompt.py --out evals/harbor/prompts/ante [--port 8765]

Point an agent's OpenAI-compatible base URL at http://127.0.0.1:<port>/v1 and run it
once with a short prompt. Every chat request is saved as request-<n>.json in --out;
the first one with tools also gives system.md (the system prompt as sent) and
tools.json (each tool's name, description and parameters). The reply is a short
"Done." so the agent ends at once. Stops after --max-requests requests (default 3)
or on Ctrl-C. See capture-prompts.sh for ForgeCLI, Forge Code and Ante.
"""

import argparse
import json
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

REPLY = "Done."


def system_text(body: dict) -> str:
    """The system prompt of a chat request: system (and developer) messages, joined."""
    parts = []
    for m in body.get("messages") or []:
        if m.get("role") in ("system", "developer"):
            content = m.get("content")
            if isinstance(content, list):
                content = "\n".join(c.get("text", "") for c in content if isinstance(c, dict))
            parts.append(str(content or ""))
    if isinstance(body.get("instructions"), str):  # Responses API
        parts.append(body["instructions"])
    return "\n\n---\n\n".join(parts)


def make_handler(out: Path, state: dict):
    class Handler(BaseHTTPRequestHandler):
        def log_message(self, fmt, *args):
            print(f"  {self.command} {self.path}", file=sys.stderr)

        def send_json(self, data: dict, code: int = 200):
            raw = json.dumps(data).encode()
            self.send_response(code)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(raw)))
            self.end_headers()
            self.wfile.write(raw)

        def do_GET(self):
            if self.path.rstrip("/").endswith("/models"):
                model = state.get("model") or "capture"
                self.send_json({"object": "list", "data": [{"id": model, "object": "model", "owned_by": "capture"}]})
            else:
                self.send_json({"error": {"message": "not found"}}, 404)

        def do_POST(self):
            length = int(self.headers.get("Content-Length") or 0)
            try:
                body = json.loads(self.rfile.read(length) or b"{}")
            except ValueError:
                body = {}
            state["n"] += 1
            n = state["n"]
            model = body.get("model") or "capture"
            state["model"] = model
            (out / f"request-{n}.json").write_text(json.dumps(body, indent=1, ensure_ascii=False))
            if body.get("tools") and not state.get("saved"):
                state["saved"] = True
                (out / "system.md").write_text(system_text(body))
                tools = [
                    {
                        "name": (t.get("function") or t).get("name"),
                        "description": (t.get("function") or t).get("description"),
                        "parameters": (t.get("function") or t).get("parameters"),
                    }
                    for t in body["tools"]
                ]
                (out / "tools.json").write_text(json.dumps(tools, indent=1, ensure_ascii=False))
                print(f"saved system.md ({len(system_text(body).split())} words) and tools.json ({len(tools)} tools)")
            created = int(time.time())
            if body.get("stream"):
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.send_header("Cache-Control", "no-cache")
                self.end_headers()
                base = {"id": f"cap-{n}", "object": "chat.completion.chunk", "created": created, "model": model}
                chunks = [
                    {**base, "choices": [{"index": 0, "delta": {"role": "assistant", "content": REPLY}, "finish_reason": None}]},
                    {**base, "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]},
                    {**base, "choices": [], "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}},
                ]
                for c in chunks:
                    self.wfile.write(f"data: {json.dumps(c)}\n\n".encode())
                self.wfile.write(b"data: [DONE]\n\n")
                self.wfile.flush()
            else:
                self.send_json({
                    "id": f"cap-{n}",
                    "object": "chat.completion",
                    "created": created,
                    "model": model,
                    "choices": [{"index": 0, "message": {"role": "assistant", "content": REPLY}, "finish_reason": "stop"}],
                    "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2},
                })
            if n >= state["max"]:
                state["stop"] = True

    return Handler


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--out", required=True, help="directory for request-*.json, system.md, tools.json")
    ap.add_argument("--port", type=int, default=8765)
    ap.add_argument("--max-requests", type=int, default=3)
    a = ap.parse_args()
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    state = {"n": 0, "max": a.max_requests, "stop": False}
    server = ThreadingHTTPServer(("127.0.0.1", a.port), make_handler(out, state))
    server.timeout = 0.5
    print(f"capturing on http://127.0.0.1:{a.port}/v1 into {out}", file=sys.stderr)
    try:
        while not state["stop"]:
            server.handle_request()
    except KeyboardInterrupt:
        pass


if __name__ == "__main__":
    main()
