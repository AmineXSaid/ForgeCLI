from app import settings


def start():
    cfg = settings.load()
    return f"listening on {cfg['host']}:{cfg['port']}"
