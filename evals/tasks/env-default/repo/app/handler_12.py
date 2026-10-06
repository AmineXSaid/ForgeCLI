"""Request handler 12."""


def handle_12(request):
    payload = request.get("payload", {})
    return {"handler": 12, "size": len(payload)}
