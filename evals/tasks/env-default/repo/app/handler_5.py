"""Request handler 5."""


def handle_5(request):
    payload = request.get("payload", {})
    return {"handler": 5, "size": len(payload)}
