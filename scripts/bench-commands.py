#!/usr/bin/env python3
"""Times the agent's `bash` calls against a scripted Claude provider, nothing sent anywhere else.

The provider's first reply carries `--ballast` MB of text, which the agent keeps in memory as
a long session would, then `--rounds` pairs of replies calling `bash` with `true`, once and
`--calls` times. Each request re-sends that text, so a call's cost is the difference between a
pair's rounds, from the reply to the next request, divided by the calls added: its start, run
and end, which a fork of the whole agent per command makes grow with the agent's memory.

    python3 scripts/bench-commands.py [--binary target/release/scramjet] [--ballast 200]

At 200 MB (about 1 GB at its peak) a run takes about 20 seconds.
"""

import argparse
import http.server
import json
import os
import shutil
import subprocess
import tempfile
import threading
import time


def sse(events):
    body = "".join(f"event: {e['type']}\ndata: {json.dumps(e)}\n\n" for e in events)
    return body.encode()


def reply(blocks, stop):
    events = [{"type": "message_start",
               "message": {"id": "msg", "usage": {"input_tokens": 1, "output_tokens": 1}}}]
    for index, (start, deltas) in enumerate(blocks):
        events.append({"type": "content_block_start", "index": index, "content_block": start})
        for delta in deltas:
            events.append({"type": "content_block_delta", "index": index, "delta": delta})
        events.append({"type": "content_block_stop", "index": index})
    events.append({"type": "message_delta", "delta": {"stop_reason": stop},
                   "usage": {"output_tokens": 1}})
    events.append({"type": "message_stop"})
    return sse(events)


def text(words, piece=1 << 20):
    pieces = [words[at:at + piece] for at in range(0, len(words), piece)]
    return ({"type": "text", "text": ""}, [{"type": "text_delta", "text": p} for p in pieces])


def calls(round_, count):
    return [({"type": "tool_use", "id": f"toolu_{round_}_{n}", "name": "bash", "input": {}},
             [{"type": "input_json_delta", "partial_json": json.dumps({"command": "true"})}])
            for n in range(count)]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", default="target/release/scramjet")
    parser.add_argument("--ballast", type=int, default=200, help="MB of text in the first reply")
    parser.add_argument("--rounds", type=int, default=2)
    parser.add_argument("--calls", type=int, default=400)
    options = parser.parse_args()

    ballast = "x" * (options.ballast << 20)
    replies = [reply([text(ballast), *calls(0, 1)], "tool_use")]
    for r in range(1, options.rounds + 1):
        replies += [reply(calls(2 * r, 1), "tool_use"), reply(calls(2 * r + 1, options.calls), "tool_use")]
    replies.append(reply([text("Done.")], "end_turn"))
    sent = []

    class Provider(http.server.BaseHTTPRequestHandler):
        def do_POST(self):
            sent.append(time.monotonic())
            remaining = int(self.headers["content-length"])
            while remaining:
                remaining -= len(self.rfile.read(min(remaining, 1 << 20)))
            body = replies[min(len(sent), len(replies)) - 1]
            self.send_response(200)
            self.send_header("content-type", "text/event-stream")
            self.send_header("content-length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def log_message(self, *_):
            pass

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Provider)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    scratch = tempfile.mkdtemp(prefix="scramjet-bench-")
    try:
        config = os.path.join(scratch, "config", "scramjet")
        os.makedirs(config)
        project = os.path.join(scratch, "project")
        os.makedirs(project)
        with open(os.path.join(config, "models.json"), "w") as models:
            json.dump({"providers": {"bench": {
                "baseUrl": f"http://127.0.0.1:{server.server_address[1]}",
                "api": "anthropic-messages", "apiKey": "none",
                "models": [{"id": "claude-bench", "contextWindow": 1 << 30}]}}}, models)
        environment = dict(os.environ, XDG_CONFIG_HOME=os.path.join(scratch, "config"),
                           XDG_STATE_HOME=os.path.join(scratch, "state"),
                           XDG_DATA_HOME=os.path.join(scratch, "data"))
        started = time.monotonic()
        agent = subprocess.run(
            ["/usr/bin/time", "-f", "%M", os.path.abspath(options.binary), "-p", "--no-session",
             "--model", "bench/claude-bench", "Run the commands."],
            cwd=project, env=environment, capture_output=True, text=True, timeout=600)
        total = time.monotonic() - started
    finally:
        server.shutdown()
        shutil.rmtree(scratch)
    if agent.returncode != 0 or len(sent) != len(replies):
        raise SystemExit(f"agent failed ({agent.returncode}, {len(sent)} requests):\n"
                         f"{agent.stdout}{agent.stderr}")
    peak = int(agent.stderr.strip().splitlines()[-1]) >> 10
    rounds = [later - earlier for earlier, later in zip(sent[1:], sent[2:])]
    per_call = sorted((many - one) / (options.calls - 1) * 1000
                      for one, many in zip(rounds[0::2], rounds[1::2]))
    print(f"{options.binary}: peak {peak} MB, {options.rounds}×{options.calls} calls, "
          f"{', '.join(f'{c:.2f}' for c in per_call)} ms per call, {total:.1f} s in all")


if __name__ == "__main__":
    main()
