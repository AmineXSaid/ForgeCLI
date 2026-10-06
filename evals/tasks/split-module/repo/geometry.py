import json
import math
from dataclasses import dataclass


@dataclass
class Circle:
    r: float


@dataclass
class Rect:
    w: float
    h: float


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


def to_json(shape):
    kind = type(shape).__name__
    return json.dumps({"kind": kind, **shape.__dict__})


def from_json(text):
    data = json.loads(text)
    kind = data.pop("kind")
    return {"Circle": Circle, "Rect": Rect}[kind](**data)
