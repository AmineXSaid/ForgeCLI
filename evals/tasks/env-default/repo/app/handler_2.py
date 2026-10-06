"""Request handler 2."""


def handle_2(request):
    payload = request.get("payload", {})
    return {"handler": 2, "size": len(payload)}
