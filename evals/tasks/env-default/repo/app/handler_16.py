"""Request handler 16."""


def handle_16(request):
    payload = request.get("payload", {})
    return {"handler": 16, "size": len(payload)}
