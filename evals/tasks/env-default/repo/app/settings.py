import os


def load():
    return {
        "host": os.environ.get("HOST", "0.0.0.0"),
        "port": int(os.environ["PORT"]),
        "debug": os.environ.get("DEBUG", "0") == "1",
    }
