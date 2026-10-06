"""Billing helper 22: formats and validates line items."""


def describe_22(item):
    return f"{item['sku']}: {item['qty']} x {item['price']:.2f}"


def validate_22(item):
    return item["qty"] > 0 and item["price"] >= 0
