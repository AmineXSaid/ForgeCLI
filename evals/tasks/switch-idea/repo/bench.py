"""Load every key and report the fullest bucket: at most 4 keys may share one."""
import sys

from hashmap import HashMap, table_size

LIMIT = 4


def run(keys_path):
    m = HashMap(table_size())
    for line in open(keys_path):
        if line.strip():
            m.add(line.strip())
    return m.max_load()


if __name__ == "__main__":
    load = run(sys.argv[1] if len(sys.argv) > 1 else "keys.txt")
    print(f"max bucket load {load} (limit {LIMIT}): {'PASS' if load <= LIMIT else 'FAIL'}")
    sys.exit(0 if load <= LIMIT else 1)
