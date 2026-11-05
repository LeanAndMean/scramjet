#!/usr/bin/env python3
"""An MCP server over standard input and output for scramjet-rs's tests, standard library only.

It speaks the `initialize` handshake (2025-11-25 and before) and answers `server/discover` as an
unknown method, as servers written before 2026-07-28 do. `MCP_TEST_LOG` names a file its process
id and each message's method are appended to; `MCP_TEST_MODE` is `exit` (it fails at once) or `silent` (it
never answers)."""
import json
import os
import sys
import threading
import time

PNG = (
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg=="
)

TOOLS = [
    {"name": "echo", "description": "Repeat the text it is given.",
     "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}},
                     "required": ["text"]}},
    {"name": "add", "title": "Add numbers", "description": "Add two numbers.",
     "inputSchema": {"type": "object", "properties": {"a": {"type": "number"},
                                                      "b": {"type": "number"}}}},
    {"name": "forecast", "description": "Tell the weather forecast for a city.",
     "inputSchema": {"type": "object", "properties": {"city": {"type": "string"}}}},
    {"name": "fail", "description": "Always reports an error.",
     "inputSchema": {"type": "object"}},
    {"name": "slow", "description": "Waits a minute unless cancelled.",
     "inputSchema": {"type": "object"}},
    {"name": "picture", "description": "Returns a tiny image.",
     "inputSchema": {"type": "object"}},
    {"name": "grow", "description": "Adds the tool `extra` to the list.",
     "inputSchema": {"type": "object"}},
]
PAGE = 3

lock = threading.Lock()
cancelled = set()


def log(method):
    path = os.environ.get("MCP_TEST_LOG")
    if path:
        with open(path, "a") as file:
            file.write(method + "\n")


def send(message):
    with lock:
        sys.stdout.write(json.dumps(message) + "\n")
        sys.stdout.flush()


def answer(id, result):
    send({"jsonrpc": "2.0", "id": id, "result": result})


def text(value, error=False):
    result = {"content": [{"type": "text", "text": value}]}
    if error:
        result["isError"] = True
    return result


def call(id, name, arguments):
    if name == "echo":
        return answer(id, text("echo: " + arguments.get("text", "")))
    if name == "add":
        total = arguments.get("a", 0) + arguments.get("b", 0)
        return answer(id, {"content": [], "structuredContent": {"sum": total}})
    if name == "forecast":
        return answer(id, text("Sunny in " + arguments.get("city", "?")))
    if name == "fail":
        return answer(id, text("it failed", error=True))
    if name == "picture":
        return answer(id, {"content": [{"type": "text", "text": "a dot"},
                                       {"type": "image", "data": PNG, "mimeType": "image/png"}]})
    if name == "grow":
        TOOLS.append({"name": "extra", "description": "Added later.",
                      "inputSchema": {"type": "object"}})
        send({"jsonrpc": "2.0", "method": "notifications/tools/list_changed"})
        return answer(id, text("grown"))
    if name == "slow":
        def wait():
            for _ in range(600):
                if id in cancelled:
                    return
                time.sleep(0.1)
            answer(id, text("done waiting"))
        return threading.Thread(target=wait, daemon=True).start()
    send({"jsonrpc": "2.0", "id": id,
          "error": {"code": -32602, "message": "Unknown tool: " + name}})


def main():
    mode = os.environ.get("MCP_TEST_MODE", "")
    log("pid %d" % os.getpid())
    sys.stderr.write("test server starting\n")
    sys.stderr.flush()
    if mode == "exit":
        sys.stderr.write("no configuration found\n")
        sys.exit(3)
    for line in sys.stdin:
        message = json.loads(line)
        method = message.get("method")
        if method is None:
            continue
        log(method)
        if mode == "silent":
            continue
        id = message.get("id")
        params = message.get("params") or {}
        if method == "initialize":
            answer(id, {"protocolVersion": "2025-11-25",
                        "capabilities": {"tools": {"listChanged": True}},
                        "serverInfo": {"name": "test", "version": "1.0.0",
                                       "description": "Tools for scramjet-rs's tests"},
                        "instructions": "Use echo to repeat text."})
        elif method == "tools/list":
            start = int(params.get("cursor") or 0)
            page = {"tools": TOOLS[start:start + PAGE]}
            if start + PAGE < len(TOOLS):
                page["nextCursor"] = str(start + PAGE)
            answer(id, page)
        elif method == "tools/call":
            call(id, params.get("name"), params.get("arguments") or {})
        elif method == "ping":
            answer(id, {})
        elif method == "notifications/cancelled":
            cancelled.add(params.get("requestId"))
        elif id is not None:
            send({"jsonrpc": "2.0", "id": id,
                  "error": {"code": -32601, "message": "Method not found: " + method}})


main()
