import sys
sys.path.insert(0, ".")
from billing.checkout import checkout
items = [{"sku": "A", "qty": 1, "price": 19.99}]
assert checkout({"id": 1, "loyalty_tier": "loyal"}, items) == 17.99, checkout({"id": 1, "loyalty_tier": "loyal"}, items)
assert checkout({"id": 2}, items) == 19.99
assert checkout({"id": 3, "loyalty_tier": "LOYAL"}, items) == 17.99
print("hidden checks passed")
