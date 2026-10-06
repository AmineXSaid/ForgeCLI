import json
import os
import subprocess
import tempfile

data = "id,label,value\n1,\"a, quoted\",3.25\n2,plain,\n3,,-4\n"
with tempfile.NamedTemporaryFile("w", suffix=".csv", delete=False) as f:
    f.write(data)
out = subprocess.run(["python3", "convert.py", f.name], capture_output=True, text=True, check=True).stdout
rows = json.loads(out)
assert rows == [
    {"id": 1, "label": "a, quoted", "value": 3.25},
    {"id": 2, "label": "plain", "value": None},
    {"id": 3, "label": None, "value": -4},
], rows
os.unlink(f.name)
print("hidden checks passed")
