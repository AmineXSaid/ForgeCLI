import configparser
import sys

cfg = configparser.ConfigParser()
cfg.read("settings.ini")
ok = True
for i in range(1, 6001):
    print(f"INFO record {i:05d}: processed batch={i % 17} shard={i % 5} status=ok")
    if i == 3127:
        retries = cfg.get("worker", "retries", fallback=None)
        if retries is None or not retries.isdigit():
            print(f"ERROR record {i:05d}: settings.ini [worker] retries must be an integer, got {retries!r}")
            ok = False
print("INFO done" if ok else "INFO finished with errors")
sys.exit(0 if ok else 1)
