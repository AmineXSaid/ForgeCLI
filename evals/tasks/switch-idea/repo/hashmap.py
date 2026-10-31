"""A small chained hash table of strings."""


def table_size(path="config.txt"):
    for line in open(path):
        name, _, value = line.partition("=")
        if name.strip() == "size":
            return int(value)
    raise ValueError("no size in config.txt")


class HashMap:
    def __init__(self, size):
        self.size = size
        self.buckets = [[] for _ in range(size)]

    def bucket(self, key):
        """The index of the bucket `key` goes to."""
        return sum(ord(c) for c in key) % self.size

    def add(self, key):
        b = self.buckets[self.bucket(key)]
        if key not in b:
            b.append(key)

    def max_load(self):
        return max(len(b) for b in self.buckets)
