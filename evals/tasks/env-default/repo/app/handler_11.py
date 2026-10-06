"""Request handler 11."""


def handle_11(request):
    payload = request.get("payload", {})
    return {"handler": 11, "size": len(payload)}
