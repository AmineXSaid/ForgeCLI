"""Pack item weights into bins of capacity 100.

Usage: python3 pack.py items.txt
Prints one bin per line, its item weights separated by spaces.
"""
import sys

CAPACITY = 100


def pack(weights):
    """Next fit: keep one bin open and start a new one when the next item doesn't fit."""
    bins = [[]]
    for w in weights:
        if sum(bins[-1]) + w > CAPACITY:
            bins.append([])
        bins[-1].append(w)
    return bins


if __name__ == "__main__":
    weights = [int(line) for line in open(sys.argv[1]) if line.strip()]
    for b in pack(weights):
        print(" ".join(map(str, b)))
