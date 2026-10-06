from shop import pricing


def invoice_lines(items, tax_rate):
    lines = [f"{qty} x {price:.2f}" for price, qty in items]
    lines.append(f"TOTAL {pricing.calc_total(items, tax_rate):.2f}")
    return lines
