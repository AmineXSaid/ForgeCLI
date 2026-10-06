from invoice import subtotal


def export_total(invoices):
    """Sum many invoices' subtotals; rounding happens once, here."""
    return round(sum(subtotal(lines) for lines in invoices), 2)
