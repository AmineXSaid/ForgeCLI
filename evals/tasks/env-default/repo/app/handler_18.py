"""Request handler 18."""


def handle_18(request):
    payload = request.get("payload", {})
    return {"handler": 18, "size": len(payload)}
