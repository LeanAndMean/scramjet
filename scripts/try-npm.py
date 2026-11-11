#!/usr/bin/env python3
"""Installs dist/npm's tarballs (scripts/npm-packages.py --pack) from a scripted registry on
127.0.0.1, as `npm install -g @leanandmean/scramjet` would from npm's, into a scratch prefix
over a stand-in of Scramjet 1's package, then runs the installed `scramjet`:

    scripts/try-npm.py

Needs node and npm; nothing is fetched from or sent to npm's registry.
"""

import base64
import hashlib
import http.server
import io
import json
import os
import shutil
import signal
import subprocess
import sys
import tarfile
import tempfile
import threading
import time
from pathlib import Path
from urllib.parse import unquote

ROOT = Path(__file__).resolve().parent.parent
NPM = ROOT / "dist" / "npm"
V1 = "0.117.0"


def manifest_of(tarball):
    with tarfile.open(fileobj=io.BytesIO(tarball)) as tar:
        return json.load(tar.extractfile("package/package.json"))


def stand_in_v1():
    """Scramjet 1's package as far as npm sees it: its name, version and two programs."""
    files = {
        "package/package.json": json.dumps(
            {
                "name": "@leanandmean/scramjet",
                "version": V1,
                "bin": {"scramjet": "bin/scramjet.js", "scramjet-command-lint": "bin/lint.js"},
            }
        ),
        "package/bin/scramjet.js": "#!/usr/bin/env node\nconsole.log('scramjet v1');\n",
        "package/bin/lint.js": "#!/usr/bin/env node\nconsole.log('lint v1');\n",
    }
    buffer = io.BytesIO()
    with tarfile.open(fileobj=buffer, mode="w:gz") as tar:
        for name, text in files.items():
            data = text.encode()
            info = tarfile.TarInfo(name)
            info.size, info.mode = len(data), 0o755
            tar.addfile(info, io.BytesIO(data))
    return buffer.getvalue()


def registry(tarballs):
    """name -> {version: (manifest, bytes)}, served as packuments and tarballs."""
    packages = {}
    for data in tarballs:
        manifest = manifest_of(data)
        packages.setdefault(manifest["name"], {})[manifest["version"]] = (manifest, data)
    requests = []

    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *args):
            pass

        def do_GET(self):
            path = unquote(self.path.split("?")[0]).lstrip("/")
            requests.append(path)
            base = f"http://{self.headers['Host']}"
            if path.endswith(".tgz"):
                name, version = path[: -len(".tgz")].rsplit("/-/", 1)
                found = packages.get(name, {}).get(version.rsplit("-", 1)[-1])
                return self.reply(found and found[1], "application/octet-stream")
            versions = packages.get(path)
            if versions is None:
                return self.reply(None, "")
            listed = {}
            for version, (manifest, data) in versions.items():
                integrity = base64.b64encode(hashlib.sha512(data).digest()).decode()
                listed[version] = dict(
                    manifest,
                    dist={
                        "tarball": f"{base}/{path}/-/{path.split('/')[-1]}-{version}.tgz",
                        "shasum": hashlib.sha1(data).hexdigest(),
                        "integrity": f"sha512-{integrity}",
                    },
                )
            latest = max(versions, key=lambda v: tuple(int(p) for p in v.split(".")))
            body = {"name": path, "dist-tags": {"latest": latest}, "versions": listed}
            self.reply(json.dumps(body).encode(), "application/json")

        def reply(self, body, kind):
            if body is None:
                self.send_response(404)
                body, kind = b'{"error":"not found"}', "application/json"
            else:
                self.send_response(200)
            self.send_header("Content-Type", kind)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server, packages, requests


def main():
    tarballs = sorted(NPM.glob("*.tgz"))
    if not tarballs:
        sys.exit("no tarballs in dist/npm: run scripts/npm-packages.py --pack first")
    datas = [path.read_bytes() for path in tarballs]
    version = next(manifest_of(d)["version"] for d in datas if "-" not in manifest_of(d)["name"].split("/")[1])
    with tempfile.TemporaryDirectory(prefix="scramjet-npm-") as scratch:
        scratch = Path(scratch)
        prefix = scratch / "prefix"
        env = dict(
            os.environ,
            npm_config_cache=str(scratch / "cache"),
            npm_config_userconfig=str(scratch / "npmrc"),
            npm_config_update_notifier="false",
            npm_config_audit="false",
            npm_config_fund="false",
            HOME=str(scratch / "home"),
            XDG_CONFIG_HOME=str(scratch / "home" / ".config"),
            XDG_STATE_HOME=str(scratch / "home" / ".local" / "state"),
            XDG_DATA_HOME=str(scratch / "home" / ".local" / "share"),
            SCRAMJET_OFFLINE="1",
        )
        (scratch / "home").mkdir()
        bin_dir = prefix / ("" if os.name == "nt" else "bin")
        program = bin_dir / "scramjet"
        failures = []

        def check(what, ok, detail=""):
            print(("PASS " if ok else "FAIL ") + what + (f": {detail}" if detail and not ok else ""))
            if not ok:
                failures.append(what)

        def install(server, spec="@leanandmean/scramjet"):
            url = f"http://127.0.0.1:{server.server_address[1]}/"
            return subprocess.run(
                ["npm", "install", "-g", "--prefix", str(prefix), "--registry", url, spec],
                env=env, capture_output=True, text=True,
            )

        # Scramjet 1 as installed today, then `npm install -g` of the newest, as its update runs.
        server, _, _ = registry([stand_in_v1()])
        done = install(server)
        server.shutdown()
        check("Scramjet 1's stand-in installs", done.returncode == 0, done.stderr)
        ran = subprocess.run([str(program)], env=env, capture_output=True, text=True)
        check("it runs as v1", ran.stdout.strip() == "scramjet v1", ran.stdout + ran.stderr)

        server, _, requests = registry([stand_in_v1(), *datas])
        done = install(server)
        check("the update to v2 installs", done.returncode == 0, done.stderr)
        fetched = [r for r in requests if r.endswith(".tgz")]
        check(
            "only this platform's program is fetched",
            len(fetched) == 2 and any(f"scramjet-{sys.platform}" in r for r in fetched),
            str(fetched),
        )
        ran = subprocess.run([str(program), "--version"], env=env, capture_output=True, text=True)
        check("`scramjet --version` is v2's", version in ran.stdout, ran.stdout + ran.stderr)
        check("v1's scramjet-command-lint is gone", not (bin_dir / "scramjet-command-lint").exists())
        ran = subprocess.run([str(program), "--no-such-option"], env=env, capture_output=True, text=True)
        check("an error's exit code comes through", ran.returncode == 1, f"{ran.returncode} {ran.stderr}")
        ran = subprocess.run([str(program), "update", "self"], env=env, capture_output=True, text=True)
        check(
            "`update self` says to update through npm",
            ran.returncode != 0 and "npm install -g @leanandmean/scramjet" in ran.stderr,
            ran.stdout + ran.stderr,
        )

        if os.name != "nt":
            # A signal sent to the launcher alone reaches the program, whose death it mirrors.
            child = subprocess.Popen(
                [str(program), "-p", "--no-session", "-"],
                env=env,
                stdin=subprocess.PIPE, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
            )
            time.sleep(1)
            child.send_signal(signal.SIGTERM)
            try:
                code = child.wait(10)
            except subprocess.TimeoutExpired:
                child.kill()
                code = None
            check("SIGTERM to the launcher ends both", code is not None, str(code))
            left = subprocess.run(
                ["pgrep", "-f", str(prefix / "lib")], capture_output=True, text=True
            ).stdout.split()
            check("no program is left running", not left, str(left))

        # Without the platform's package (npm 11 keeps it even with --omit=optional when
        # installing globally, but a local install leaves it out) the launcher says what to do.
        server.shutdown()
        modules = prefix / ("node_modules" if os.name == "nt" else "lib/node_modules")
        for platform in modules.glob("@leanandmean/scramjet/node_modules/@leanandmean/*"):
            shutil.rmtree(platform)
        ran = subprocess.run([str(program), "--version"], env=env, capture_output=True, text=True)
        check(
            "without its platform's package it says how to install again",
            ran.returncode == 1 and "install again" in ran.stderr,
            f"{ran.returncode} {ran.stdout} {ran.stderr}",
        )
    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()
