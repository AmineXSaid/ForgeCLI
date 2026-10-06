from billing.customers import load_customer
from billing.discounts import loyalty_discount


def checkout(record, items):
    customer = load_customer(record)
    subtotal = round(sum(i["price"] * i["qty"] for i in items), 2)
    return round(subtotal - loyalty_discount(customer, subtotal), 2)
