from shop.pricing import calc_total as total_of


def daily_report(orders):
    return {name: total_of(items) for name, items in orders.items()}
