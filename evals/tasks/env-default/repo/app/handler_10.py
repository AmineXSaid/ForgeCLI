"""Request handler 10."""


def handle_10(request):
    payload = request.get("payload", {})
    return {"handler": 10, "size": len(payload)}
