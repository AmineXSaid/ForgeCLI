cat > convert.py <<'PY'
import csv
import json
import sys


def convert(cell):
    if cell == "":
        return None
    for kind in (int, float):
        try:
            return kind(cell)
        except ValueError:
            pass
    return cell


with open(sys.argv[1], newline="") as f:
    rows = [{k: convert(v) for k, v in row.items()} for row in csv.DictReader(f)]
print(json.dumps(rows))
PY
