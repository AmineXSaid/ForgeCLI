"""Request handler 13."""


def handle_13(request):
    payload = request.get("payload", {})
    return {"handler": 13, "size": len(payload)}
