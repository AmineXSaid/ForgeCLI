#!/usr/bin/env python3
"""Fill a uv cache, once, with what a task's tests download, for runs on a slow network.

    python3 evals/harbor/warm_uv_cache.py pytorch-model-cli [more-tasks...]
    export FORGE_HARBOR_UV_CACHE=~/.forge-harbor/uv-cache

Terminal-Bench tests install their tools with `uvx` (pytorch-model-cli: Torch,
torchvision, OpenCV, numpy, pytest) every time they run. Through a slow company
proxy that download alone can outlast the task's verifier limit, and the task
scores 0 whatever the agent did. This script reads each task's own test script,
runs its `uvx` line once on this machine (same uv version, same packages, with
the tool's `--version` instead of the tests), and keeps what uv downloaded in
FORGE_HARBOR_UV_CACHE (default ~/.forge-harbor/uv-cache). With that variable set,
the agent copies the cache into each task container and the run scripts point
the tests' uv at it (UV_CACHE_DIR, see verifier-ca.sh), so the packages come
from disk. uv still asks the index whether they changed, which is small.

The task scripts come from the Terminal-Bench 2.0 repository at the commit the
dataset pins (FORGE_HARBOR_TB2_COMMIT to change it). With FORGE_HARBOR_CA_BUNDLE
set, uv on this machine trusts it too.
"""

import os
import re
import shlex
import shutil
import subprocess
import sys
import urllib.request
from pathlib import Path

REPO = "https://raw.githubusercontent.com/laude-institute/terminal-bench-2"
COMMIT = os.environ.get("FORGE_HARBOR_TB2_COMMIT") or "69671fbaac6d67a7ef0dfec016cc38a64ef7a77c"
# uvx options that take a value; anything else starting with "-" is a flag.
WITH_VALUE = {
    "-p", "--python", "-w", "--with", "--from", "--index", "--default-index", "--index-url",
    "--extra-index-url", "--index-strategy", "--with-requirements", "--find-links", "-f",
}


def test_script(task: str) -> str:
    url = f"{REPO}/{COMMIT}/{task}/tests/test.sh"
    with urllib.request.urlopen(url, timeout=60) as r:
        return r.read().decode()


def uvx_line(script: str) -> list[str] | None:
    """The script's `uvx ...` command as arguments, its backslash continuations joined."""
    joined = re.sub(r"\\\n", " ", script)
    for line in joined.splitlines():
        words = shlex.split(line, comments=True)
        if words and words[0] == "uvx":
            return words
    return None


def warm_args(words: list[str]) -> list[str]:
    """uvx's options as the test gives them, then its tool with `--version` instead of the tests."""
    out, i = [], 1
    while i < len(words):
        w = words[i]
        if w in WITH_VALUE:
            out += words[i : i + 2]
            i += 2
        elif w.startswith("-"):
            out.append(w)
            i += 1
        else:
            return out + [w, "--version"]
    raise ValueError("no tool after uvx's options")


def uv_version(script: str) -> str | None:
    """The uv version the script installs (astral.sh/uv/<version>/install.sh), if it pins one."""
    m = re.search(r"astral\.sh/uv/([0-9][0-9.]*)/install\.sh", script)
    return m.group(1) if m else None


def prune_environments(cache: Path) -> int:
    """Drop what only works on this machine, keep the downloaded packages.

    uvx also caches the environments it builds, whose bin/python links to this machine's
    interpreter; docker cp copies such a link as it is, and in a container it would dangle.
    The environments are cheap to rebuild from the cached packages, so they go, with any link
    left pointing at nothing.
    """
    root = cache.resolve()
    dropped = 0
    for bucket in cache.glob("archive-v*"):
        for entry in bucket.iterdir():
            links = (p for p in entry.rglob("*") if p.is_symlink())
            if any(not p.resolve().is_relative_to(root) for p in links):
                shutil.rmtree(entry)
                dropped += 1
    for p in list(cache.rglob("*")):
        if p.is_symlink() and not p.exists():
            p.unlink()
    return dropped


def main() -> None:
    tasks = sys.argv[1:]
    if not tasks:
        sys.exit(__doc__)
    cache = Path(os.environ.get("FORGE_HARBOR_UV_CACHE") or Path.home() / ".forge-harbor" / "uv-cache").expanduser()
    cache.mkdir(parents=True, exist_ok=True)
    env = {k: v for k, v in os.environ.items() if k != "UV_CACHE_DIR"}
    ca = os.environ.get("FORGE_HARBOR_CA_BUNDLE")
    if ca:
        env["SSL_CERT_FILE"] = str(Path(ca).resolve())
    failed = []
    for task in tasks:
        script = test_script(task)
        words = uvx_line(script)
        if not words:
            print(f"{task}: its tests don't use uvx, nothing to cache")
            continue
        version = uv_version(script)
        # The tests' own uv version, so the cache has the layout that version reads.
        # The outer uvx only fetches that uv; it keeps it in its usual cache, not this one.
        runner = ["uvx", "--from", f"uv=={version}", "uvx"] if version else ["uvx"]
        cmd = runner + ["--cache-dir", str(cache)] + warm_args(words)
        print(f"{task}: {shlex.join(cmd)}", flush=True)
        if subprocess.run(cmd, env=env).returncode != 0:
            failed.append(task)
    dropped = prune_environments(cache)
    size = sum(p.stat().st_size for p in cache.rglob("*") if p.is_file() and not p.is_symlink())
    print(f"\ncache: {cache} ({size / 1e9:.1f} GB)")
    if dropped:
        print(f"dropped {dropped} environments built on this machine's Python (uv rebuilds them in the container)")
    if failed:
        sys.exit(f"failed: {' '.join(failed)}")
    print(f"next: export FORGE_HARBOR_UV_CACHE={cache}")


if __name__ == "__main__":
    main()
