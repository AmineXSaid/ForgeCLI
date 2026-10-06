"""Request handler 9."""


def handle_9(request):
    payload = request.get("payload", {})
    return {"handler": 9, "size": len(payload)}
