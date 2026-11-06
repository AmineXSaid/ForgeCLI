# Fix, don't fit

Read this before every fix to KodAct. It is the rule `KODACT.md` sends every
change through, next to "measure, don't assume" in `docs/GOALS.md`.

KodAct runs on whatever model the person chooses. Today that may be a 27B
model behind a company gateway, with a 100k window and a 32k reply cap.
Tomorrow it may be a large model with a window of 200k to 1M tokens and
longer replies. A problem found on one model, in one run or on one benchmark
task is fixed in the harness, for every model. KodAct is never tuned to the
case that showed it.

Before proposing or making a fix, check it against each point:

1. **Limits come from the model.** The context window, the reply cap and the
   time left are read from the model's limits (`modelLimits`, what the
   endpoint reports, the settings) and from the run. Thresholds are shares of
   them. A constant sized for 100k or 32k is a fit.
2. **It holds on a large model.** Say how the fix behaves on a large window
   and a strong, fast model: it helps there or stays out of the way. A fix
   that throttles a model that has room is a fit: lower caps, earlier
   compaction, shorter replies, fewer tokens kept.
3. **It fixes a mechanism, not a task.** The cause is named in what KodAct
   does or fails to do, and the fix applies to any project. No task names,
   file names or benchmark details in code, prompts or tests' logic.
4. **The evidence spans cases.** It is checked across the tasks and runs
   that show the problem, both ways: the failure goes away, and what worked
   still works. One trial is a lead, not a reason.
5. **Tests cover both ends.** Where a limit is involved, the tests run the
   fix with a small and a large window (or reply cap), and the large case
   behaves as before.
6. **Prompts speak to every model.** A prompt rule is good practice for any
   model, worded as such, not a workaround for one model's habits.

A proposal states, next to its effect in real use, how it behaves on a large
model.

## How it is checked without a large model at hand

`scripts/check-window-scaling.py` runs the real `kodact` binary over HTTP
against a local endpoint that plays the model, once with a 1,000,000-token
window and once with a 100,000-token one. The endpoint refuses, as vLLM
does, any request whose prompt and requested reply exceed its window. No API
key is needed; run it after any change to limits, compaction or requests:

```bash
cargo build -p kodact-cli && scripts/check-window-scaling.py target/debug/kodact
```

It passes when:
- on the large window, nothing is compacted and every request asks for the
  full reply cap, however long the conversation gets within the window;
- on the small window, the conversation is compacted, the requests after it
  carry the person's request word for word, a request near the window asks
  for no more than the room left, and the endpoint refuses nothing;
- `--max-time` stops a slow run near its limit with `stop_reason: "max_time"`.

The run on 2026-11-06, at commit `1bf9a97`: on the 1M window, 7 requests
up to 77,861 tokens, no compaction, all asking 32,000. On the 100k window, the
summary call came at 78,135 tokens and asked 20,532 instead of 32,000, so it
fit; the two requests after the compaction carried the request word for word;
nothing was refused. `--max-time 3` stopped after 3.0 s with `max_time`.

This checks KodAct's behaviour on a large window, not a large model's task
success: that is measured with `kodact-eval` on the model itself.
