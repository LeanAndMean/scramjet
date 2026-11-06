#!/usr/bin/env python3
"""An MCP server over streamable HTTP for scramjet-rs's tests, standard library only.

It listens on 127.0.0.1, on port `MCP_TEST_PORT` or a free one, and prints its port on a line;
in TLS with `MCP_TEST_TLS`'s directory's `localhost.pem` and `localhost.key` (tests/tls). It speaks the `initialize`
handshake of 2025-11-25 with a session, and refuses `server/discover` outside one with HTTP 400,
as the TypeScript SDK's servers do. It answers `tools/list` in JSON and `tools/call` as events.
Each request is appended to the file `MCP_TEST_LOG` names: its method, path, JSON-RPC method,
session and protocol version. `MCP_TEST_TOKEN` is the bearer token it asks for (HTTP 401
without it). `/old` redirects to `/mcp` with a 307 and its URL, `/moved` with a 308 and its
path, `/away` with a 307 to localhost, `/gone` with a 301, each keeping the query."""
import http.server
import json
import os
import ssl
import threading
import time
import uuid

TOOLS = [
    {"name": "echo", "description": "Repeat the text it is given.",
     "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}}},
    {"name": "fail", "description": "Always reports an error.",
     "inputSchema": {"type": "object"}},
    {"name": "slow", "description": "Waits a minute unless cancelled.",
     "inputSchema": {"type": "object"}},
]

REDIRECTS = {
    "/old": (307, "http://127.0.0.1:{port}/mcp"),
    "/moved": (308, "/mcp"),
    "/away": (307, "http://localhost:{port}/mcp"),
    "/gone": (301, "/mcp"),
}

lock = threading.Lock()
sessions = set()
cancelled = set()


def log(line):
    path = os.environ.get("MCP_TEST_LOG")
    if path:
        with lock, open(path, "a") as file:
            file.write(line + "\n")


def result(id, value):
    return {"jsonrpc": "2.0", "id": id, "result": value}


def text(value, error=False):
    answer = {"content": [{"type": "text", "text": value}]}
    if error:
        answer["isError"] = True
    return answer


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def logged(self, method):
        session = self.headers.get("Mcp-Session-Id") or "-"
        version = self.headers.get("MCP-Protocol-Version") or "-"
        log("%s %s %s session=%s version=%s" % (self.command, self.path, method,
                                               "known" if session in sessions else session,
                                               version))

    def authorized(self):
        token = os.environ.get("MCP_TEST_TOKEN")
        if not token or self.headers.get("Authorization") == "Bearer " + token:
            return True
        self.reply(401, {"error": "unauthorized"},
                   [("WWW-Authenticate", 'Bearer resource_metadata="http://127.0.0.1/meta"')])
        return False

    def reply(self, status, body=None, headers=()):
        data = b"" if body is None else json.dumps(body).encode()
        self.send_response(status)
        for name, value in headers:
            self.send_header(name, value)
        if body is not None:
            self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def redirected(self):
        path, _, query = self.path.partition("?")
        if path not in REDIRECTS:
            return False
        self.rfile.read(int(self.headers.get("Content-Length") or 0))
        self.logged("redirect")
        status, to = REDIRECTS[path]
        to = to.format(port=self.server.server_address[1]) + ("?" + query if query else "")
        self.reply(status, None, [("Location", to)])
        return True

    def events(self, messages):
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Cache-Control", "no-cache")
        self.end_headers()
        for message in messages:
            self.event(message)

    def event(self, message):
        self.wfile.write(("event: message\ndata: %s\n\n" % json.dumps(message)).encode())
        self.wfile.flush()

    def do_GET(self):
        if self.redirected():
            return
        self.logged("stream")
        self.reply(405)

    def do_DELETE(self):
        if self.redirected():
            return
        self.logged("end")
        sessions.discard(self.headers.get("Mcp-Session-Id"))
        self.reply(200)

    def do_POST(self):
        if self.redirected():
            return
        message = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        method = message.get("method", "reply")
        self.logged(method)
        if not self.authorized():
            return
        id = message.get("id")
        session = self.headers.get("Mcp-Session-Id")
        if method == "initialize":
            session = uuid.uuid4().hex
            sessions.add(session)
            return self.reply(200, result(id, {
                "protocolVersion": "2025-11-25",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "remote", "version": "1.0.0",
                               "description": "Remote tools for scramjet-rs's tests"}}),
                [("Mcp-Session-Id", session)])
        if session not in sessions:
            return self.reply(400 if session is None else 404, {
                "jsonrpc": "2.0", "id": None,
                "error": {"code": -32000, "message": "Bad Request: Server not initialized"}})
        if id is None:
            if method == "notifications/cancelled":
                cancelled.add(message["params"]["requestId"])
            return self.reply(202)
        if method == "tools/list":
            return self.reply(200, result(id, {"tools": TOOLS}))
        if method == "tools/call":
            name = message["params"]["name"]
            arguments = message["params"].get("arguments") or {}
            if name == "echo":
                return self.events([result(id, text("echo: " + arguments.get("text", "")))])
            if name == "fail":
                return self.events([result(id, text("it failed", error=True))])
            if name == "slow":
                self.events([])
                for _ in range(600):
                    if id in cancelled:
                        return
                    time.sleep(0.1)
                return self.event(result(id, text("done waiting")))
        self.reply(200, {"jsonrpc": "2.0", "id": id,
                         "error": {"code": -32601, "message": "Method not found: " + method}})


server = http.server.ThreadingHTTPServer(
    ("127.0.0.1", int(os.environ.get("MCP_TEST_PORT") or 0)), Handler)
server.daemon_threads = True
tls = os.environ.get("MCP_TEST_TLS")
if tls:
    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(os.path.join(tls, "localhost.pem"), os.path.join(tls, "localhost.key"))
    server.socket = context.wrap_socket(server.socket, server_side=True)
print(server.server_address[1], flush=True)
server.serve_forever()
