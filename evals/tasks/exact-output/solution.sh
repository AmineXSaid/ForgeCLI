cat > stats.py <<'PY'
import csv

order, values = [], {}
with open("data/readings.csv") as f:
    for row in csv.DictReader(f):
        s = row["sensor"]
        if s not in values:
            order.append(s)
            values[s] = []
        values[s].append(float(row["celsius"]))
for s in order:
    print(f"Sensor {s} mean: {sum(values[s]) / len(values[s]):.1f}")
PY
