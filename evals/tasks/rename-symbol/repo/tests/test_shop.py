import sys
import unittest

sys.path.insert(0, ".")

from shop.cart import Cart
from shop.invoice import invoice_lines
from report import daily_report


class ShopTest(unittest.TestCase):
    def test_cart(self):
        c = Cart()
        c.add(2.5, 2)
        c.add(1.0)
        self.assertEqual(c.total(0.1), 6.6)

    def test_invoice(self):
        self.assertEqual(invoice_lines([(1.0, 2)], 0.0)[-1], "TOTAL 2.00")

    def test_report(self):
        self.assertEqual(daily_report({"a": [(3.0, 1)]}), {"a": 3.0})


if __name__ == "__main__":
    unittest.main()
