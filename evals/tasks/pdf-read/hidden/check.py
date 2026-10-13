from pathlib import Path

answer = Path("answer.txt").read_text().strip().replace(",", "")
assert answer in ("5500", "5500.00"), f"answer.txt holds {answer!r}, not the Q3 total (5500)"
print("hidden checks passed")
