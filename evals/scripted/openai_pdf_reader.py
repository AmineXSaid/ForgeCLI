"""A scripted OpenAI chat-completions model that knows nothing about the task's answer.

It reads budget.pdf with Read, looks for the Q3 forecast in what the harness hands back,
and writes it to answer.txt only if it finds it there. So the eval passes only when the
harness actually delivers the PDF's text over this wire.
"""
import json, re, sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

def chunk(delta, finish=None):
    return "data: " + json.dumps({"id": "c", "object": "chat.completion.chunk", "model": "m", "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]}) + "\n\n"

def call(name, args):
    return chunk({"role": "assistant"}) + chunk({"tool_calls": [{"index": 0, "id": f"call_{name}", "type": "function", "function": {"name": name, "arguments": json.dumps(args)}}]}) + chunk({}, "tool_calls")

def say(text):
    return chunk({"role": "assistant", "content": text}) + chunk({}, "stop")

class H(BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_GET(self):
        b = json.dumps({"data": [{"id": "m"}]}).encode(); self.send_response(200); self.end_headers(); self.wfile.write(b)
    def do_POST(self):
        req = json.loads(self.rfile.read(int(self.headers.get("content-length", 0))))
        tools = [m for m in req["messages"] if m["role"] == "tool"]
        read = next((m for m in tools if m["tool_call_id"] == "call_Read"), None)
        wrote = any(m["tool_call_id"] == "call_Write" for m in tools)
        if wrote:
            out = say("Done.")
        elif read is None:
            out = call("Read", {"file_path": "budget.pdf"})
        else:
            m = re.search(r"Q3 forecast is\s*([\d,]+)", read["content"] or "")
            out = call("Write", {"file_path": "answer.txt", "content": m.group(1).replace(",", "") + "\n"}) if m else say("I couldn't read the PDF's text.")
        self.send_response(200); self.send_header("content-type", "text/event-stream"); self.end_headers()
        self.wfile.write((out + "data: [DONE]\n\n").encode())

ThreadingHTTPServer(("127.0.0.1", int(sys.argv[1])), H).serve_forever()
