import math
import unittest

from geometry import Circle, Rect, area, from_json, perimeter, to_json


class GeometryTest(unittest.TestCase):
    def test_area(self):
        self.assertAlmostEqual(area(Circle(1)), math.pi)
        self.assertEqual(area(Rect(2, 3)), 6)

    def test_perimeter(self):
        self.assertEqual(perimeter(Rect(2, 3)), 10)

    def test_round_trip(self):
        self.assertEqual(from_json(to_json(Rect(1, 2))), Rect(1, 2))


if __name__ == "__main__":
    unittest.main()
