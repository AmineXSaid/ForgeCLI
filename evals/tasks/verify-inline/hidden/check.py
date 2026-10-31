import math
import sys

sys.path.insert(0, ".")
from shapes import area

assert area("square", 3) == 9
assert area("rect", 3, 4) == 12
assert math.isclose(area("circle", 2), math.pi * 4)
try:
    area("hexagon", 1)
except ValueError:
    pass
else:
    raise AssertionError("an unknown shape must raise ValueError")
print("hidden checks passed")
