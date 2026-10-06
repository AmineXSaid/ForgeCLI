mkdir geometry
cat > geometry/shapes.py <<'PY'
from dataclasses import dataclass


@dataclass
class Circle:
    r: float


@dataclass
class Rect:
    w: float
    h: float
PY
cat > geometry/metrics.py <<'PY'
import math

from geometry.shapes import Circle, Rect


def area(shape):
    if isinstance(shape, Circle):
        return math.pi * shape.r ** 2
    if isinstance(shape, Rect):
        return shape.w * shape.h
    raise TypeError(f"unknown shape {shape!r}")


def perimeter(shape):
    if isinstance(shape, Circle):
        return 2 * math.pi * shape.r
    if isinstance(shape, Rect):
        return 2 * (shape.w + shape.h)
    raise TypeError(f"unknown shape {shape!r}")
PY
cat > geometry/io.py <<'PY'
import json

from geometry.shapes import Circle, Rect


def to_json(shape):
    return json.dumps({"kind": type(shape).__name__, **shape.__dict__})


def from_json(text):
    data = json.loads(text)
    kind = data.pop("kind")
    return {"Circle": Circle, "Rect": Rect}[kind](**data)
PY
cat > geometry/__init__.py <<'PY'
from geometry.io import from_json, to_json
from geometry.metrics import area, perimeter
from geometry.shapes import Circle, Rect

__all__ = ["Circle", "Rect", "area", "perimeter", "to_json", "from_json"]
PY
rm geometry.py
