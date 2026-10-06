import configparser

cfg = configparser.ConfigParser()
cfg.read("settings.ini")
assert cfg.get("worker", "retries").isdigit(), "retries must be an integer"
assert cfg.get("worker", "threads") == "4" and cfg.get("worker", "timeout") == "30", "other settings unchanged"
print("hidden checks passed")
