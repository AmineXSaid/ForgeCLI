import sys
sys.path.insert(0, ".")
from shop.pricing import compute_total
assert compute_total([(10.0, 3)], 0.5) == 45.0
print("hidden checks passed")
