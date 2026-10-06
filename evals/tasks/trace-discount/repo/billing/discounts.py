LOYALTY_RATE = 0.10


def loyalty_discount(customer, subtotal):
    if customer.get("tier") == "loyal":
        return round(subtotal * LOYALTY_RATE, 2)
    return 0.0
