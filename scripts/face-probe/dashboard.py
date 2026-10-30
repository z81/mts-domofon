#!/usr/bin/env python3
"""Local-only debug UI and collector supervisor. Camera capture is opt-in."""

import argparse
from functools import partial
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import math
import mimetypes
import os
from pathlib import Path
import secrets
import signal
import threading
from urllib.parse import parse_qs, unquote, urlsplit

from assets import ROOT
from dashboard_worker import Collector
from watch_stream import base_origin


WEB = Path(__file__).resolve().parent / "web"
SECURITY_HEADERS = {
    "Cache-Control": "no-store",
    "X-Content-Type-Options": "nosniff",
    "Referrer-Policy": "no-referrer",
    "X-Frame-Options": "DENY",
    "Content-Security-Policy": "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self'; "
                               "object-src 'none'; frame-ancestors 'none'; base-uri 'none'; form-action 'self'",
}


class Handler(BaseHTTPRequestHandler):
    server_version = "FaceProbe"

    def __init__(self, *args, collector, token, allowed_hosts, **kwargs):
        self.collector, self.token, self.allowed_hosts = collector, token, allowed_hosts
        super().__init__(*args, **kwargs)

    def setup(self):
        super().setup()
        self.connection.settimeout(10)

    def log_message(self, format, *args):
        # No request bodies, report paths, reference names or session tokens in container logs.
        pass

    def respond(self, status, value):
        payload = json.dumps(value, ensure_ascii=False, allow_nan=False).encode("utf-8")
        self.headers_for(status, "application/json; charset=utf-8", len(payload))
        self.wfile.write(payload)

    def headers_for(self, status, content_type, length):
        self.send_response(status)
        for name, value in SECURITY_HEADERS.items():
            self.send_header(name, value)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(length))
        self.end_headers()

    def trusted_request(self, *, mutation=False):
        host = self.headers.get("Host", "")
        try:
            parsed = urlsplit("//" + host)
            if (parsed.hostname not in self.allowed_hosts or parsed.username or parsed.password
                    or parsed.path or parsed.query or parsed.fragment):
                return False
            _ = parsed.port
            origin = self.headers.get("Origin")
            if origin:
                source = urlsplit(origin)
                if source.scheme not in ("http", "https") or source.netloc != host or source.path not in ("", "/"):
                    return False
        except ValueError:
            return False
        if self.headers.get("Sec-Fetch-Site") == "cross-site":
            return False
        return not mutation or secrets.compare_digest(self.headers.get("X-Face-Probe-Token", ""), self.token)

    def do_GET(self):
        try:
            if not self.trusted_request():
                self.respond(403, {"error": "Untrusted host or origin"})
                return
            parsed = urlsplit(self.path)
            path = unquote(parsed.path)
            if path == "/health":
                self.respond(200, {"status": "ok", "doorActionsEnabled": False})
            elif path == "/api/state":
                selected = parse_qs(parsed.query).get("run", [None])[0]
                state = self.collector.state(selected)
                self.respond(200, {**state, "csrfToken": self.token})
            elif path.startswith("/reports/"):
                parts = path.removeprefix("/reports/").split("/", 1)
                if len(parts) != 2:
                    raise FileNotFoundError("Report file not found")
                self.send_file(self.collector.reports.asset(*parts))
            elif path in ("/", "/app.js", "/style.css"):
                self.send_file(WEB / ("index.html" if path == "/" else path[1:]))
            else:
                self.respond(404, {"error": "Not found"})
        except FileNotFoundError as error:
            self.respond(404, {"error": str(error)})
        except ValueError as error:
            self.respond(400, {"error": str(error)})
        except (BrokenPipeError, ConnectionResetError, TimeoutError):
            pass
        except OSError:
            self.respond(500, {"error": "Cannot read local report"})

    def send_file(self, path):
        content_type = {".jsonl": "application/x-ndjson", ".log": "text/plain; charset=utf-8"}.get(path.suffix)
        content_type = content_type or mimetypes.guess_type(path.name)[0] or "application/octet-stream"
        with path.open("rb") as source:
            remaining = os.fstat(source.fileno()).st_size
            self.headers_for(200, content_type, remaining)
            while remaining:
                chunk = source.read(min(remaining, 64_000))
                if not chunk:
                    break
                self.wfile.write(chunk)
                remaining -= len(chunk)

    def do_POST(self):
        try:
            if not self.trusted_request(mutation=True):
                self.respond(403, {"error": "Untrusted request or missing CSRF token"})
                return
            if self.headers.get("Content-Type", "").split(";", 1)[0] != "application/json":
                self.respond(415, {"error": "Expected application/json"})
                return
            length = int(self.headers.get("Content-Length", "0"))
            if not 0 < length <= 4096 or self.headers.get("Transfer-Encoding"):
                self.respond(413, {"error": "Request body must be 1..4096 bytes"})
                return
            data = json.loads(self.rfile.read(length))
            if self.path == "/api/start":
                self.respond(202, {"runId": self.collector.start(data)})
            elif self.path == "/api/stop":
                self.collector.stop()
                self.respond(202, {"status": "stopping"})
            else:
                self.respond(404, {"error": "Not found"})
        except ValueError as error:
            self.respond(400, {"error": str(error)})
        except RuntimeError as error:
            self.respond(409, {"error": str(error)})
        except (BrokenPipeError, ConnectionResetError, TimeoutError):
            pass
        except OSError:
            self.respond(500, {"error": "Cannot start collector or write reports"})


def arguments(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=8765)
    parser.add_argument("--allowed-host", action="append", default=[], help="Additional trusted UI hostname, not a URL")
    parser.add_argument("--base-url", default=os.getenv("FACE_PROBE_BASE_URL", "http://127.0.0.1:3000"))
    parser.add_argument("--model-root", type=Path, default=Path(os.getenv("FACE_PROBE_MODELS", str(ROOT))))
    parser.add_argument("--references", type=Path, default=Path(os.getenv("FACE_PROBE_REFERENCES", str(ROOT.parent / "references"))))
    parser.add_argument("--data-dir", type=Path, default=Path(os.getenv("FACE_PROBE_DATA", str(ROOT.parent / "reports"))))
    parser.add_argument("--max-mb", type=float, default=1024)
    parser.add_argument("--min-free-mb", type=float, default=256)
    parser.add_argument("--threads", type=int, default=3)
    args = parser.parse_args(argv)
    args.base_url = base_origin(args.base_url)
    if not 1 <= args.port <= 65535 or not 1 <= args.threads <= 64:
        parser.error("Invalid port or CPU thread count")
    if not math.isfinite(args.max_mb) or args.max_mb < 1 or not math.isfinite(args.min_free_mb) or args.min_free_mb < 0:
        parser.error("Invalid storage limits")
    for name in ("model_root", "references", "data_dir"):
        setattr(args, name, getattr(args, name).expanduser().resolve())
    return args


def main(argv=None):
    args = arguments(argv)
    os.umask(0o077)
    collector = Collector(args)
    server = None
    try:
        handler = partial(Handler, collector=collector, token=secrets.token_urlsafe(32),
                          allowed_hosts={"localhost", "127.0.0.1", "::1", *args.allowed_host})
        server = ThreadingHTTPServer((args.host, args.port), handler)
        server.daemon_threads = True

        def stop(signum, frame):
            threading.Thread(target=server.shutdown, daemon=True).start()

        signal.signal(signal.SIGINT, stop)
        signal.signal(signal.SIGTERM, stop)
        print(f"Face-probe UI listening on {args.host}:{args.port}; capture stopped; door control absent.", flush=True)
        server.serve_forever(poll_interval=.25)
    finally:
        if server:
            server.server_close()
        collector.close()


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError) as error:
        raise SystemExit(f"Error: {error}") from None
