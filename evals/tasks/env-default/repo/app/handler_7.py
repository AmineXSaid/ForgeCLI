"""Request handler 7."""


def handle_7(request):
    payload = request.get("payload", {})
    return {"handler": 7, "size": len(payload)}
