"""Pi's JSON event stream (`pi --mode json`), as the stream-json events ForgeCLI writes.

pi_agent.py saves Pi's events in `agent/pi.jsonl`. report.py and watch.py read
ForgeCLI's `forge.jsonl` (assistant, user, system and result events); this module
turns Pi's events into the same shape, so turns, tokens, tool calls and errors
are counted and shown the same way for both harnesses. Tool names stay Pi's own
(`bash`, `read`, `edit`, `write`).

    stream = PiStream()
    for line in lines:
        for event in stream.feed(json.loads(line)):
            ...
"""


class PiStream:
    """Feed Pi's events in order; each call returns the stream-json events they amount to."""

    def __init__(self):
        self.turns = 0
        self.tokens = {"input_tokens": 0, "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0, "output_tokens": 0}
        self.cost = 0.0
        self.last = {}
        self.first_ms = None
        self.retries = 0

    def feed(self, event: dict) -> list[dict]:
        kind = event.get("type")
        if kind == "message_end":
            message = event.get("message") or {}
            if message.get("role") == "assistant":
                return [self.assistant(message)]
            if message.get("role") == "toolResult":
                block = {
                    "type": "tool_result",
                    "tool_use_id": message.get("toolCallId"),
                    "content": message.get("content") or [],
                    "is_error": bool(message.get("isError")),
                }
                return [{"type": "user", "message": {"role": "user", "content": [block]}}]
        elif kind == "auto_retry_start":
            self.retries += 1
            return [{
                "type": "system",
                "subtype": "api_retry",
                "attempt": event.get("attempt"),
                "retry_delay_ms": event.get("delayMs"),
                "error": event.get("errorMessage") or "",
            }]
        elif kind == "compaction_end":
            return [{"type": "system", "subtype": "compaction", "reason": event.get("reason"), "aborted": event.get("aborted")}]
        elif kind == "agent_end":
            return [self.result()]
        return []

    def assistant(self, message: dict) -> dict:
        # A failed model call ends in an empty assistant message: not a turn of work.
        if message.get("content") or message.get("stopReason") != "error":
            self.turns += 1
        self.last = message
        stamp = message.get("timestamp")
        if isinstance(stamp, (int, float)) and self.first_ms is None:
            self.first_ms = stamp
        usage = message.get("usage") or {}
        step = {
            "input_tokens": usage.get("input") or 0,
            "cache_read_input_tokens": usage.get("cacheRead") or 0,
            "cache_creation_input_tokens": usage.get("cacheWrite") or 0,
            "output_tokens": usage.get("output") or 0,
        }
        for k, v in step.items():
            self.tokens[k] += v
        self.cost += (usage.get("cost") or {}).get("total") or 0
        content = []
        for block in message.get("content") or []:
            t = block.get("type")
            if t == "text":
                content.append({"type": "text", "text": block.get("text", "")})
            elif t == "thinking":
                content.append({"type": "thinking", "thinking": block.get("thinking", "")})
            elif t == "toolCall":
                content.append({"type": "tool_use", "id": block.get("id"), "name": block.get("name"), "input": block.get("arguments")})
        return {"type": "assistant", "message": {"role": "assistant", "content": content, "usage": step, "stop_reason": message.get("stopReason")}}

    def result(self) -> dict:
        """Pi's run ended: how, as ForgeCLI's final `result` line says it."""
        stop = self.last.get("stopReason") or ""
        error = self.last.get("errorMessage") or ""
        text = next((b.get("text", "") for b in reversed(self.last.get("content") or []) if b.get("type") == "text"), "")
        end = self.last.get("timestamp")
        duration = (end - self.first_ms) if isinstance(end, (int, float)) and self.first_ms is not None else 0
        return {
            "type": "result",
            "subtype": {"stop": "success", "aborted": "aborted"}.get(stop, "error_during_execution"),
            "is_error": stop != "stop",
            "num_turns": self.turns,
            "duration_ms": duration + ((self.last.get("durationMs") or 0) if self.last else 0),
            "stop_reason": "api_error" if stop == "error" else stop,
            "errors": [f"API Error: {error}"] if stop == "error" and error else ([error] if error else []),
            "result": text,
            "usage": dict(self.tokens),
            "total_cost_usd": self.cost,
        }
