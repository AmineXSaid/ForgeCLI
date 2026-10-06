"""Request handler 3."""


def handle_3(request):
    payload = request.get("payload", {})
    return {"handler": 3, "size": len(payload)}
