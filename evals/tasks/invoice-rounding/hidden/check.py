import sys

sys.path.insert(0, ".")
from invoice import invoice_total, line_amount, subtotal

assert invoice_total([(1, 0.1), (2, 0.1)], 0.0) == 0.3
assert invoice_total([(4, 2.5)], 0.075) == 10.75
assert invoice_total([(1, 19.99)], 0.0) == 19.99
assert abs(line_amount(3, 0.335) - 1.005) < 1e-9, "line amounts keep full precision"
assert abs(subtotal([(1, 0.005)] * 3) - 0.015) < 1e-9, "subtotals keep full precision"
print("hidden checks passed")
