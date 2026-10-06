def load_customer(record):
    return {"id": record["id"], "tier": record.get("loyalty_tier", "standard").upper()}
