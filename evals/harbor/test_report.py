"""Tests for report.py's verdicts, run without Harbor, Docker or an endpoint.

    python3 -m pytest evals/harbor/test_report.py

The one that matters here: a trial where the verifier's own test phase could not
fetch its packages or data (a cert/network failure behind a TLS-inspecting proxy)
is set apart as "verifier error", not counted as the agent failing, while a trial
whose tests actually ran and failed stays "false success". The fixtures are cut
down from real terminal-bench 2.0 verifier logs (pytorch-model-cli, write-compressor,
gpt2-codegolf) on the KPIT endpoint.
"""

import json
from pathlib import Path

import report


def make_trial(tmp: Path, task: str, reward: str, verifier_stdout: str, *, ran_tests: int | None) -> report.Trial:
    """A minimal trial dir: the agent said done (a forge result), the verifier wrote `reward`."""
    d = tmp / f"{task}__TEST"
    (d / "agent").mkdir(parents=True)
    (d / "verifier").mkdir(parents=True)
    # ForgeCLI's stream with a clean end: this is the "said done" case a false success comes from.
    result = {"type": "result", "subtype": "success", "stop_reason": "end_turn", "num_turns": 3}
    (d / "agent" / "forge.jsonl").write_text(json.dumps(result) + "\n")
    (d / "verifier" / "reward.txt").write_text(reward)
    (d / "verifier" / "test-stdout.txt").write_text(verifier_stdout)
    if ran_tests is not None:  # a ctrf.json as json-ctrf would write it
        summary = {"tests": ran_tests, "passed": 0, "failed": ran_tests, "skipped": 0}
        (d / "verifier" / "ctrf.json").write_text(json.dumps({"results": {"summary": summary, "tests": []}}))
    return report.Trial(d)


# A verifier that died fetching its own dependencies: uv could not reach download.pytorch.org
# because the proxy's CA is not trusted in the test container, so no test body ran.
PYTORCH_INFRA = """\
Hit:1 http://deb.debian.org/debian bookworm InRelease
Reading package lists...
/tests/test.sh: line 9: /root/.local/bin/env: No such file or directory
error: Failed to fetch: `https://download-r2.pytorch.org/whl/cpu/torchvision-0.22.1%2Bcpu-cp313-cp313-manylinux_2_28_x86_64.whl.metadata`
  Caused by: Request failed after 3 retries
  Caused by: error sending request for url (https://download-r2.pytorch.org/whl/cpu/torchvision-0.22.1%2Bcpu...whl.metadata)
  Caused by: invalid peer certificate: UnknownIssuer
"""

# A verifier whose setup printed a transient cert warning, but then the tests ran and the agent's
# output genuinely failed them (pytest reached the test bodies: "3 failed").
WRITE_COMPRESSOR_REAL_FAIL = """\
Setting up ca-certificates (20260601~24.04.1) ...
curl: (60) SSL certificate problem: unable to get local issuer certificate
Installed 6 packages in 19ms
============================= test session starts ==============================
collected 3 items
../tests/test_outputs.py FFF                                             [100%]
=========================== short test summary info ============================
FAILED ../tests/test_outputs.py::test_compressed_file_exists - AssertionError
============================== 3 failed in 0.22s ===============================
"""

# A plain wrong answer: no network trouble at all, tests ran, one failed.
GPT2_REAL_FAIL = """\
============================= test session starts ==============================
collected 1 item
../tests/test_outputs.py F                                               [100%]
FAILED ../tests/test_outputs.py::test_gpt2_implementation - AssertionError
============================== 1 failed in 0.07s ===============================
"""


def test_pytorch_verifier_cert_failure_is_verifier_error(tmp_path):
    t = make_trial(tmp_path, "pytorch-model-cli", "0.0", PYTORCH_INFRA, ran_tests=None)
    assert t.verdict == "verifier error"
    assert t.verifier_infra is True


def test_tests_that_ran_and_failed_stay_false_success(tmp_path):
    # Setup had a cert hiccup, but the tests reached their bodies: this is the agent's failure.
    t = make_trial(tmp_path, "write-compressor", "0.0", WRITE_COMPRESSOR_REAL_FAIL, ran_tests=3)
    assert t.verdict == "false success"
    assert t.verifier_infra is False


def test_plain_wrong_answer_is_false_success(tmp_path):
    t = make_trial(tmp_path, "gpt2-codegolf", "0.0", GPT2_REAL_FAIL, ran_tests=1)
    assert t.verdict == "false success"


def test_verifier_error_is_left_out_of_valid_trials(tmp_path):
    trials = [
        make_trial(tmp_path / "a", "pytorch-model-cli", "0.0", PYTORCH_INFRA, ran_tests=None),
        make_trial(tmp_path / "b", "gpt2-codegolf", "0.0", GPT2_REAL_FAIL, ran_tests=1),
    ]
    invalid = ("api error", "verifier error")
    valid = [t for t in trials if not t.verdict.startswith(invalid)]
    # Only the genuine failure is a valid trial; the cert-failed verifier is set apart.
    assert [t.task for t in valid] == ["gpt2-codegolf"]


def test_verifier_timeout_is_verifier_error(tmp_path):
    # The tests ran out of time (e.g. downloading Torch through a slow proxy) and wrote no reward:
    # nothing about the agent was measured.
    t = make_trial(tmp_path, "pytorch-model-cli", "", "", ran_tests=None)
    t.exception = "VerifierTimeoutError"
    assert t.decide() == "verifier error"
