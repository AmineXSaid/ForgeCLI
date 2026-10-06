"""Request handler 17."""


def handle_17(request):
    payload = request.get("payload", {})
    return {"handler": 17, "size": len(payload)}
