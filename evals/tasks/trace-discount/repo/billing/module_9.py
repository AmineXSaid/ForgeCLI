"""Billing helper 9: formats and validates line items."""


def describe_9(item):
    return f"{item['sku']}: {item['qty']} x {item['price']:.2f}"


def validate_9(item):
    return item["qty"] > 0 and item["price"] >= 0
