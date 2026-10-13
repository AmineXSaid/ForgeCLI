# Scripted models

Models that know nothing about a task's answer, for measuring the harness
itself when no API key is at hand. A task passes only if ForgeCLI hands the
model what it needs.

## `openai_pdf_reader.py`: the `pdf-read` task on an OpenAI-compatible endpoint

An OpenAI chat-completions endpoint that does three things:
1. reads `budget.pdf` with Read;
2. looks for the Q3 forecast in the tool result;
3. writes it to `answer.txt` only if it finds it there.

```bash
python3 evals/scripted/openai_pdf_reader.py 8791 &
export FORGE_OPENAI_BASE_URL=http://127.0.0.1:8791/v1 FORGE_OPENAI_API_KEY=sk-test0123456789
forge-eval run --only pdf-read --forge <forge> --label <label> --repeat 3 -- --model m --permission-mode bypassPermissions
```

Measured on 2026-10-13. A is 1610f82, before the OpenAI document fix; B is that fix.

| Metric | A | B |
| --- | --- | --- |
| Pass rate | 0% | 100% |
| False-finish rate | 100% | 0% |
| Turns per task | 2.0 | 4.0 |

Before the fix, the Read result reached the model as the PDF's base64, so it could never find the figure.

A real-model comparison still needs an API key.
