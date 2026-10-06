def calc_total(items, tax_rate=0.0):
    """Sum of price * quantity, plus tax."""
    subtotal = sum(price * qty for price, qty in items)
    return round(subtotal * (1 + tax_rate), 2)
