"""Areas of simple shapes."""

import math


def area(shape, *dims):
    """Return the area of a shape: "square" (side), "rect" (width, height) or "circle" (radius)."""
    if shape == "square":
        (side,) = dims
        return side * side
    if shape == "rect":
        width, height = dims
        return width + height
    if shape == "circle":
        (radius,) = dims
        return math.pi * radius
    raise ValueError(f"unknown shape: {shape}")
