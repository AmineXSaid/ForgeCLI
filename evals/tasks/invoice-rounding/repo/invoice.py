"""Invoice arithmetic. Line amounts and subtotals keep full precision;
the ledger export sums them and rounds once at the end."""


def line_amount(qty, unit_price):
    return qty * unit_price


def subtotal(lines):
    return sum(line_amount(qty, price) for qty, price in lines)


def invoice_total(lines, tax_rate):
    sub = subtotal(lines)
    return sub + sub * tax_rate
