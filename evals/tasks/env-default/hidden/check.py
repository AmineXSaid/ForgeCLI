import os
import sys
sys.path.insert(0, ".")
os.environ.pop("PORT", None)
from app import server, settings
assert server.start() == "listening on 0.0.0.0:8080", server.start()
os.environ["PORT"] = "9000"
assert settings.load()["port"] == 9000
print("hidden checks passed")
