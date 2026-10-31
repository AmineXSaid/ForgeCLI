#!/usr/bin/env python3
"""Download, once, the packages an agent may `pip install`, for runs on a slow network.

    python3 evals/harbor/warm_pip_cache.py            # Torch (CPU), the usual one
    python3 evals/harbor/warm_pip_cache.py torch torchvision
    export FORGE_HARBOR_PIP_CACHE=~/.forge-harbor/pip-wheelhouse

Some tasks (pytorch-model-cli) do not give the agent a package it wants, so the agent installs it
itself with `pip install`. Through a slow proxy that download can outlast the task's time limit, or
fall back to the far larger CUDA build and never finish. This fetches the CPU wheels once into a
wheelhouse (FORGE_HARBOR_PIP_CACHE, default ~/.forge-harbor/pip-wheelhouse). With that variable set,
every agent copies the wheelhouse into its container and pip is pointed there (PIP_FIND_LINKS), so
the agent's own `pip install torch` comes from disk; anything not cached still falls back to the
index. The task still withholds the package — only the download speed changes — and it applies to
every agent, so comparisons stay even.

With FORGE_HARBOR_CA_BUNDLE set, pip here trusts it too. Jobs that use the wheelhouse get -pipcache.
"""

import os
import subprocess
import sys
from pathlib import Path

ATTEMPTS = 3
TORCH_CPU_INDEX = "https://download.pytorch.org/whl/cpu"


def main() -> None:
    pkgs = sys.argv[1:] or ["torch"]
    house = Path(os.environ.get("FORGE_HARBOR_PIP_CACHE") or Path.home() / ".forge-harbor" / "pip-wheelhouse")
    house = house.expanduser()
    house.mkdir(parents=True, exist_ok=True)
    env = {k: v for k, v in os.environ.items() if not k.startswith("PIP_")}
    env["PIP_DEFAULT_TIMEOUT"] = "600"  # a slow proxy needs far longer than pip's default
    ca = os.environ.get("FORGE_HARBOR_CA_BUNDLE")
    if ca:
        env["PIP_CERT"] = str(Path(ca).resolve())
    # The CPU index holds the Torch wheels; its dependencies come from PyPI. Asking for the CPU
    # build keeps this to ~180 MB instead of the multi-GB CUDA build with its NVIDIA packages.
    cmd = [
        sys.executable, "-m", "pip", "download", *pkgs,
        "--index-url", TORCH_CPU_INDEX,
        "--extra-index-url", "https://pypi.org/simple",
        "-d", str(house),
    ]
    print(" ".join(cmd), flush=True)
    for attempt in range(1, ATTEMPTS + 1):
        if subprocess.run(cmd, env=env).returncode == 0:
            break
        print(f"attempt {attempt} of {ATTEMPTS} failed" + ("; trying again" if attempt < ATTEMPTS else ""), flush=True)
    else:
        sys.exit("failed: could not fill the wheelhouse")
    wheels = list(house.glob("*.whl"))
    size = sum(w.stat().st_size for w in wheels)
    print(f"\nwheelhouse: {house} ({len(wheels)} wheels, {size / 1e9:.2f} GB)")
    print(f"next: export FORGE_HARBOR_PIP_CACHE={house}")


if __name__ == "__main__":
    main()
