import sys
sys.path.insert(0, ".")
from stats import median

assert median([1, 2]) == 1.5
assert median([10, 2, 8, 4, 6, 12]) == 7
assert median([5]) == 5
try:
    median([])
except ValueError:
    pass
else:
    raise AssertionError("empty input must raise ValueError")
print("hidden checks passed")
