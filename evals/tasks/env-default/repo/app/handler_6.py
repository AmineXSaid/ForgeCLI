"""Request handler 6."""


def handle_6(request):
    payload = request.get("payload", {})
    return {"handler": 6, "size": len(payload)}
