"""Request handler 15."""


def handle_15(request):
    payload = request.get("payload", {})
    return {"handler": 15, "size": len(payload)}
