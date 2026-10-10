# ForgeCLI on Terminal-Bench (10-task subset): analysis of the run logs

Sources: `GPT.txt`, `KPIT-reasoning-text.txt`, `quick-thinking.txt`, `new 10.txt`, `ante-deepthinking.txt`,
checked against the ForgeCLI source at `c64aae5`, then against the full job folders in `jobs/` (branch FORG_BENCH).
**(inferred)** marks a deduction; everything else is quoted or computed from the logs and code.

> **Corrections after reading `jobs/`.** (1) ForgeCLI and Ante used the **same** endpoint,
> `gpt.technica-engineering.net`. The 429s say `Concurrent request limit reached. Running: 2, limit: 2`, and two
> ForgeCLI deep-thinking jobs ran at the same time (`…-122710`, 12:27-13:29, and `…-122822`), which also overlapped
> quick-thinking `…-125959`. (2) ForgeCLI does write its errors to `forge.stderr.txt`; the `watch.py` used then only
> read `forge.jsonl`. (3) The 4213b82 job's 8 errors are network errors: DNS lookup failed (3), host unreachable (1),
> connection dropped mid-reply ("error decoding response body", 4).

## 1. Scoreboard

| # | Job (model, commit, start) | File | Mean | Exceptions | Runtime |
|---|---|---|---|---|---|
| J1 | kgpt-reasoning-text be64c91, 10-08 16:00 | new 10.txt:26 | 0.2 | none | 37m |
| J2 | kgpt-reasoning-text 353b6f1, 10-08 16:44 | new 10.txt:369 | 0.0 | 2 error exits | 19m |
| J3 | quick-thinking 4213b82, 10-08 17:24 | new 10.txt:648 | 0.1 | 8 error exits (DNS failure, unreachable host, dropped connections) | 48m on the progress bar, 16h total (machine asleep overnight, inferred) |
| J4 | quick-thinking 02195fd, 10-09 09:39 | KPIT:24, quick-thinking.txt | 0.4 | 3 timeouts | 1h44m |
| J5 | kgpt-reasoning-text 02195fd, 10-09 11:52 | KPIT:112 | 0.1 | none | 23m |
| J6 | deep-thinking 02195fd, 10-09 12:28 | GPT.txt:10 | 0.1 | 9 error exits, 1 timeout | 31m |
| J7 | quick-thinking 02195fd, 10-09 12:59 | GPT.txt:41 | 0.2 | 6 error exits, 1 timeout | 1h16m |
| J8 | gpt 02195fd, 10-09 14:16 | GPT.txt:72 | 0.3 | 2 error exits, 1 timeout | 1h14m |
| A | **Ante** deep-thinking, 10-09 16:14 | ante-deepthinking.txt | 0.4 | 3 error exits, 2 timeouts | unknown (span runs overnight) |

**The "02195fd" jobs probably ran the 4213b82 binary (inferred).** `run.sh` names a job after `git rev-parse HEAD`
but runs whatever is in `target-static/` and never rebuilds it (`evals/harbor/run.sh:27-30,38`). The last
build in the logs is the 4213b82 build (new 10.txt, before line 648). The later fix for invalid tool-call
JSON (490d94c) found that these "cut off by the output token limit" errors came from calls the model ended
itself, and replaced that message for them. Yet J5 still shows it 27 times.

## 2. Task × job matrix

✓ pass · fail T timeout E error exit A API failure before any work ? no per-task data

| Task | J1 | J2 | J3 | J4 | J5 | J6 | J7 | J8 | Ante |
|---|---|---|---|---|---|---|---|---|---|
| log-summary-date-ranges | ✓ | ? | ✓ | ✓ | · | A | ? | ? | ✓ |
| merge-diff-arc-agi-task | ✓ | ? | E | ✓ | · | E | ? | ? | ✓ |
| llm-inference-batching-scheduler | · | ? | E | ✓ | · | A | ? | ? | ✓ |
| winning-avg-corewars | · | ? | E | ✓ | ✓ | A | ? | ? | E |
| break-filter-js-from-html | · | ? | E | T | · | A | ? | ? | ✓ |
| largest-eigenval | · | ? | E | T | · | ✓ (inferred) | ? | ? | E |
| pytorch-model-cli | · | ? | · | · | · | T (inferred) | ? | ? | E |
| gpt2-codegolf | · | ? | E | · | · | A | ? | ? | T |
| reshard-c4-data | · | ? | E | · | · | A | ? | ? | · |
| write-compressor | · | ? | E | T | · | A | ? | ? | T |

- Never passed anywhere: gpt2-codegolf, pytorch-model-cli, reshard-c4-data, write-compressor.
- Swing tasks: batching-scheduler, merge-diff, corewars, break-filter.
- log-summary is the most reliable, but still failed in J5 (wrong counts) and never started in J6.

## 3. Per-trial causes (jobs with transcripts)

| Job | Task | Outcome | Cause | Class |
|---|---|---|---|---|
| J6 | gpt2, batching, break-filter, reshard, write-compressor, corewars, log-summary | 0 turns, 138s each | HTTP 429 "Concurrent request limit reached. Running: 2, limit: 2" (job 122710 ran at the same time) until ForgeCLI's retry budget ran out (2+4+8+16+32+60s waits, plus 0.5+1s provider retries per attempt: `limit.rs:22-26,163-215`, `openai.rs:51`) | endpoint + harness |
| J6 | merge-diff | error after 3 turns, 392s | git missing (`git: command not found`), then the 4th model call failed and the run ended | endpoint + harness |
| J6 | largest-eigenval | 33 turns, probably the one pass | slow: dead ends on LAPACK symbols, numba, power iteration | model |
| J6 | pytorch-model-cli | 24 turns, probably the timeout | bet on installing CUDA torch; Bash 120s timeout on pip (`GPT.txt:156`) | model + harness |
| J5 | break-filter | "success", 2 turns, 3s | model refused twice ("I'm sorry, but I can't help with that", KPIT:309,313) | model, reported as success |
| J5 | write-compressor | "success", 3 turns, 30s | gave up after reading two files (KPIT:471); no reminder because tools had been used | model + harness |
| J5 | gpt2-codegolf | "success", 4 turns | declined as unrealistic; Read of the binary .ckpt failed | model + harness |
| J5 | log-summary | "success", 5 turns, reward 0 | wrong counts (ERROR 414 vs 370), never cross-checked (KPIT:710-711) | model |
| J5 | batching, reshard, merge-diff, pytorch, largest-eigenval | "success", 0 reward | solutions that fail the checker; 27 tool calls lost to "cut off by the output token limit" across the job | model + harness |
| J4 | break-filter, write-compressor, largest-eigenval | timeout | long investigations (mXSS search, porting an encoder to C without python3, porting LAPACK to C); 2-minute warning came too late (quick-thinking.txt:1013) | model + harness |
| J4 | gpt2, reshard, pytorch | "success", 0 reward | checker failed; reasons are in `verifier/test-stdout.txt` | unknown |
| A | pytorch-model-cli | error exit | "Connection reset by peer" mid-reply; Ante does not retry it | endpoint |
| A | largest-eigenval | error exit | "transport error" after 4 reconnects (1, 2, 4, 8s) | endpoint |
| A | gpt2-codegolf, write-compressor | timeout | reverse-engineering the weights layout; debugging the range coder | model |

## 4. ForgeCLI harness problems, ranked by expected gain

| # | Problem | Seen | Code | Fix | Effort |
|---|---|---|---|---|---|
| 1 | Any failed model call ends the run; 429s give up after ~138s; a reply that breaks halfway is never retried | J6: 8 of 10 trials; J3/J7/J8: 16 error exits (cause not visible) | `engine.rs:1491`, `engine.rs:795`, `limit.rs:24` | In unattended runs, retry 429/5xx/network/mid-stream failures with backoff until a reserve of `--max-time` is left | M |
| 2 | Jobs labelled with HEAD may run an older binary | all "02195fd" jobs (inferred) | `run.sh:27-38` | `run.sh` rebuilds when the binary is older than HEAD, and `forge --version` prints the commit | S |
| 3 | Give-ups and refusals end as `success` | J5: 3 trials in ≤30s | `engine.rs:1673-1684` (reminder only when no tool was used) | One "keep going or verify" reminder when the model stops early with lots of time left; report refusals and give-ups honestly; never push on a safety refusal | M |
| 4 | Tool calls lost as "cut off by the output token limit" | J5: 27, J4: 4 | `engine.rs:1572-1588`, 490d94c | Re-measure on a fresh build first; 490d94c may already fix most of them | S |
| 5 | `--max-time` is advisory: no hard stop, Bash timeouts not clamped to time left | J4: 3 timeouts; J6: pytorch ran past 900s | `engine.rs:1303-1309`, `bash.rs:248` | Hard stop at the deadline; clamp tool timeouts to remaining time; warn earlier | M |
| 6 | Why a run failed was invisible in the live view | the `watch.py` of that time read only `forge.jsonl`; the errors were in `forge.stderr.txt` | `watch.py` | Fixed: `watch.py` shows the result's error and stderr (2716dbf) | done |
| 7 | Missing tools in task containers (git, python3, file) cost turns | J5, J6, J4, Ante | `prompts/05-autonomous.md` | In `--autonomous`, list available tools at start and say "apt-get update && apt-get install -y" | S |
| 8 | Tool name in wrong case: `glob` → "No such tool available" | J5: 1 | `exec.rs:145` | Match case-insensitively, or name the right tool in the error | S |
| 9 | Read of a binary file is a dead end | J5: 1 | `read.rs:157-174` | Suggest `xxd`, `od` or Python | S |
| 10 | Bash default 120s timeout kills installs | J6: 1 | `bash.rs:292-295` | Say how to raise `timeout` or use `run_in_background` | S |

## 5. Measurement problems

- **Concurrency limit of 2 requests per key:** J6 lost 7 trials to 429 before any work because another job ran at the
  same time. Run one job at a time; with `FORGE_HARBOR_JOBS=2` a single job already uses the whole limit.
- **Machine sleep:** J3 shows 48m on the progress bar but 16h total; the Ante job also spans the night (inferred: the
  laptop slept, open connections died).
- **Same endpoint, different load:** both harnesses used `gpt.technica-engineering.net`; ForgeCLI's J6 shared it with
  another job, Ante's run did not.
- **Noise:** the same model and commit gave 0.4 (J4) and 0.2 (J7) on the same day. One attempt per task can't
  separate a 0.1 difference from noise.
- **Stale binary:** see section 1.

## 6. Ante vs ForgeCLI (deep-thinking)

| | ForgeCLI (J6) | Ante |
|---|---|---|
| Endpoint and time | gpt.technica-engineering.net, 12:28, while another ForgeCLI job ran | gpt.technica-engineering.net, 16:14, alone |
| Mean | 0.1 | 0.4 |
| Trials that reached the model | 3 of 10 | 10 of 10 |
| Trials lost to the endpoint | 8 | 2, maybe 3 |
| Retry on API failure | 429: ~138s; network: 2 quick retries; mid-reply: none | 4 reconnects over 15s; mid-reply: none |
| Background commands | model has to poll BashOutput | waits for the command's exit notice |
| Live view | thinking + raw JSON (fixed in 2716dbf) | narrative text only |

**Verdict:** this pair doesn't compare the harnesses. ForgeCLI never got to work on the 4 tasks Ante passed, because
two ForgeCLI jobs shared a 2-request limit. On a
day the endpoint answered (J4), ForgeCLI also scored 0.4. Neither harness survives a dropped connection.
ForgeCLI could take one thing from Ante: a way to wait for a background command.

## 7. Next steps

**Fix how we measure**
1. Rebuild before every run (`evals/harbor/build-static.sh`) until `run.sh` does it itself.
2. Keep the machine awake (`systemd-inhibit --what=sleep:idle evals/harbor/run.sh subset`) and run one job at a time.
3. Run Ante and ForgeCLI on the same endpoint, back to back, with `FORGE_HARBOR_JOBS=1` and `--n-attempts 3` on
   the swing tasks.

**Fix in ForgeCLI** (in this order)
4. Retry API failures in unattended runs (problem 1). Metric: trials lost to the endpoint.
5. `run.sh` checks the binary against HEAD (problem 2). Metric: job labels match what ran.
6. Show why a run ended: stderr, plus a `report.py` table per job (problem 6). Metric: time to diagnose.
7. Honest finishes: give-up reminder and result flags (problem 3). Metric: false-success rate.
8. `--max-time` hard stop and clamped tool timeouts (problem 5). Metric: AgentTimeoutError count.
9. Small tool fixes (problems 7-10). Metric: tool error rate.

## 8. Open questions and where to look

| Question | File in `jobs/<job>/<trial>/` |
|---|---|
| The exact API error behind each 0-turn and error exit | Answered above (429 concurrency limit; network errors in J3) |
| Which binary ran | `ls -l --time-style=full-iso target-static/release/forge` vs commit times |
| Why reshard, pytorch and gpt2 fail after "success" | `verifier/test-stdout.txt` |
| Whether the machine slept during J3 and Ante | `trial.log` timestamps, `journalctl -b \| grep -i suspend` |
| Which endpoint Ante used | Answered: the same one, `gpt.technica-engineering.net` |
