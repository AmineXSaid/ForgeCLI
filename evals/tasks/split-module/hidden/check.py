import sys
sys.path.insert(0, ".")
from geometry.shapes import Circle, Rect
from geometry.metrics import area, perimeter
from geometry.io import to_json, from_json
assert from_json(to_json(Circle(2.5))) == Circle(2.5)
assert perimeter(Rect(1, 1)) == 4
print("hidden checks passed")
