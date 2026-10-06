import unittest

from invoice import invoice_total, subtotal
from ledger import export_total


class InvoiceTest(unittest.TestCase):
    def test_total_is_rounded_to_cents(self):
        self.assertEqual(invoice_total([(1, 0.1), (2, 0.1)], 0.0), 0.3)

    def test_total_includes_tax(self):
        self.assertEqual(invoice_total([(2, 1.25), (1, 0.1)], 0.2), 3.12)

    def test_subtotal_keeps_full_precision(self):
        self.assertAlmostEqual(subtotal([(3, 0.335)]), 1.005, places=9)

    def test_ledger_rounds_once(self):
        # 300 lines of 0.005 each: rounding each line first would lose or invent cents.
        self.assertEqual(export_total([[(1, 0.005)]] * 300), 1.5)


if __name__ == "__main__":
    unittest.main()
