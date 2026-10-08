#!/usr/bin/env python3
"""Follow a Harbor job's ForgeCLI conversations live, as readable text.

    python3 evals/harbor/watch.py                 # the newest job under jobs/
    python3 evals/harbor/watch.py jobs/<job-name> # a given job
    python3 evals/harbor/watch.py --from-start    # replay what's there, then follow

Each task's agent writes one stream-json line per complete message, so a step
appears when the model finishes it (a reasoning model can be quiet for minutes).
Ctrl-C stops watching; the run itself isn't affected.
"""

import json
import sys
import time
from pathlib import Path

WIDTH = 400  # characters kept from thinking, tool input and tool output
COLORS = ["\033[36m", "\033[35m", "\033[33m", "\033[32m", "\033[34m", "\033[31m"]
DIM, BOLD, RESET = "\033[2m", "\033[1m", "\033[0m"


def short(text: str, width: int = WIDTH) -> str:
    text = " ".join(str(text).split())
    return text if len(text) <= width else text[: width - 1] + "…"


def render(event: dict) -> list[str]:
    kind = event.get("type")
    out = []
    if kind == "assistant":
        for block in event.get("message", {}).get("content", []):
            t = block.get("type")
            if t == "thinking" and block.get("thinking"):
                out.append(f"{DIM}thinks: {short(block['thinking'])}{RESET}")
            elif t == "text" and block.get("text"):
                out.append(f"{BOLD}says:{RESET} {short(block['text'])}")
            elif t == "tool_use":
                out.append(f"{BOLD}→ {block.get('name')}{RESET} {short(json.dumps(block.get('input', {})), 300)}")
    elif kind == "user":
        content = event.get("message", {}).get("content", [])
        for block in content if isinstance(content, list) else []:
            if block.get("type") == "tool_result":
                body = block.get("content")
                if isinstance(body, list):
                    body = " ".join(b.get("text", "") for b in body if isinstance(b, dict))
                mark = "✗" if block.get("is_error") else "←"
                out.append(f"  {mark} {short(body or '', 300)}")
            elif block.get("type") == "text" and event.get("isSynthetic"):
                out.append(f"{DIM}  [forge] {short(block.get('text', ''), 200)}{RESET}")
    elif kind == "result":
        out.append(
            f"{BOLD}== finished: {event.get('subtype')} after {event.get('num_turns')} turns, "
            f"{(event.get('duration_ms') or 0) / 1000:.0f}s{RESET}"
        )
    elif kind == "system" and event.get("subtype") not in (None, "init"):
        out.append(f"{DIM}  [system] {event.get('subtype')}{RESET}")
    return out


def newest_job() -> Path:
    jobs = [p for p in Path("jobs").glob("*") if p.is_dir()]
    if not jobs:
        sys.exit("no jobs/ directory with runs here; cd to the ForgeCLI checkout or pass a job path")
    return max(jobs, key=lambda p: p.stat().st_mtime)


def main() -> None:
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    from_start = "--from-start" in sys.argv
    job = Path(args[0]) if args else newest_job()
    print(f"watching {job}  (Ctrl-C to stop)\n")
    offsets: dict[Path, int] = {}
    colors: dict[str, str] = {}
    pending: dict[Path, str] = {}
    while True:
        for log in sorted(job.glob("*/agent/forge.jsonl")):
            if log not in offsets:
                offsets[log] = 0 if from_start else log.stat().st_size
            size = log.stat().st_size
            if size <= offsets[log]:
                continue
            with log.open(errors="replace") as f:
                f.seek(offsets[log])
                chunk = pending.pop(log, "") + f.read()
                offsets[log] = f.tell()
            lines = chunk.split("\n")
            if lines[-1]:
                pending[log] = lines[-1]  # partial line: finish it on the next read
            task = log.parent.parent.name.split("__")[0]
            color = colors.setdefault(task, COLORS[len(colors) % len(COLORS)])
            for line in lines[:-1]:
                try:
                    event = json.loads(line)
                except ValueError:
                    continue
                for text in render(event):
                    print(f"{color}[{task}]{RESET} {text}", flush=True)
        time.sleep(1)


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        pass
