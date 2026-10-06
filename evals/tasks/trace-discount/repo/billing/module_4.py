"""Billing helper 4: formats and validates line items."""


def describe_4(item):
    return f"{item['sku']}: {item['qty']} x {item['price']:.2f}"


def validate_4(item):
    return item["qty"] > 0 and item["price"] >= 0
