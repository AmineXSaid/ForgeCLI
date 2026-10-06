"""Request handler 1."""


def handle_1(request):
    payload = request.get("payload", {})
    return {"handler": 1, "size": len(payload)}
