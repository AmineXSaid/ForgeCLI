"""Request handler 8."""


def handle_8(request):
    payload = request.get("payload", {})
    return {"handler": 8, "size": len(payload)}
