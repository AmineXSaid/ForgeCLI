"""Route a request path to a backend: the first rule whose prefix matches wins."""
import sys


def load(path):
    rules = []
    for line in open(path):
        line = line.split("#", 1)[0].strip()
        if line:
            prefix, backend = line.split()
            rules.append((prefix, backend))
    return rules


def route(rules, path):
    for prefix, backend in rules:
        if path == prefix or path.startswith(prefix.rstrip("/") + "/"):
            return backend
    return "default-backend"


if __name__ == "__main__":
    print(route(load(sys.argv[1]), sys.argv[2]))
