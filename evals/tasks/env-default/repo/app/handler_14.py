"""Request handler 14."""


def handle_14(request):
    payload = request.get("payload", {})
    return {"handler": 14, "size": len(payload)}
