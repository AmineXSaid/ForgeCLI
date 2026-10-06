"""Request handler 4."""


def handle_4(request):
    payload = request.get("payload", {})
    return {"handler": 4, "size": len(payload)}
