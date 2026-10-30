"""Read the app's default HLS stream; only GET requests, no control API calls."""

from collections import deque
from datetime import datetime, timezone
from http.client import HTTPException
import re
import struct
import subprocess
import threading
import time
from urllib.error import HTTPError, URLError
from urllib.parse import urlsplit
from urllib.request import HTTPRedirectHandler, ProxyHandler, Request, build_opener


PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"
MAX_PNG_BYTES = 16 * 1024 * 1024
MAX_PIXELS = 4096 * 2160
PLAYLIST_PATH = re.compile(r"/api/v1/live/[0-9a-fA-F-]{36}/index\.m3u8\Z")


def utc_now():
    return datetime.now(timezone.utc).isoformat(timespec="milliseconds").replace("+00:00", "Z")


def base_origin(value):
    parsed = urlsplit(value)
    if (parsed.scheme not in ("http", "https") or not parsed.hostname or parsed.username or parsed.password
            or parsed.query or parsed.fragment or parsed.path not in ("", "/")):
        raise ValueError("--base-url must be an HTTP(S) origin without credentials, path or query")
    _ = parsed.port  # Validate malformed ports before starting a worker.
    return f"{parsed.scheme}://{parsed.netloc}"


def validate_playlist_url(value, origin):
    parsed, base = urlsplit(value), urlsplit(origin)
    if (parsed.scheme != base.scheme or parsed.netloc != base.netloc or parsed.query or parsed.fragment
            or not PLAYLIST_PATH.fullmatch(parsed.path)):
        raise ValueError("stream_redirect_rejected")
    return value


class MediaRedirect(HTTPRedirectHandler):
    def __init__(self, origin):
        self.origin = origin

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        validate_playlist_url(newurl, self.origin)
        return super().redirect_request(req, fp, code, msg, headers, newurl)


def resolve_playlist(origin, timeout):
    # No ambient proxy credentials, provider credentials, POSTs or DELETEs.
    opener = build_opener(ProxyHandler({}), MediaRedirect(origin))
    try:
        request = Request(origin + "/api/v1/cameras/stream", method="GET")
        with opener.open(request, timeout=timeout) as response:
            url = validate_playlist_url(response.url, origin)
            body = response.read(256 * 1024 + 1)
            if len(body) > 256 * 1024 or not body.startswith(b"#EXTM3U"):
                raise ValueError("invalid_live_playlist")
            # The backend exposes only relative segment-N.ts assets, no encryption or variants.
            for line in body.decode("utf-8").splitlines():
                if 'URI=' in line or (line and not line.startswith("#")
                                      and not re.fullmatch(r"segment-[0-9]{1,12}\.ts", line)):
                    raise ValueError("unsupported_live_playlist")
            return url
    except HTTPError as error:
        raise ValueError(f"stream_http_{error.code}") from None
    except (URLError, TimeoutError, OSError, HTTPException):
        raise ValueError("stream_request_failed_or_timed_out") from None


def read_exact(stream, length):
    chunks, remaining = [], length
    while remaining:
        chunk = stream.read(remaining)
        if not chunk:
            raise EOFError("decoder_pipe_closed")
        chunks.append(chunk)
        remaining -= len(chunk)
    return b"".join(chunks)


def read_png(stream):
    signature = read_exact(stream, 8)
    if signature != PNG_SIGNATURE:
        raise ValueError("invalid_decoder_png_signature")
    chunks, size, first = [signature], 8, True
    while True:
        header = read_exact(stream, 8)
        length, kind = struct.unpack(">I4s", header)
        size += 12 + length
        if size > MAX_PNG_BYTES:
            raise ValueError("decoder_frame_too_large")
        payload = read_exact(stream, length + 4)
        if first:
            if kind != b"IHDR" or length != 13:
                raise ValueError("invalid_decoder_png_header")
            width, height = struct.unpack(">II", payload[:8])
            if not width or not height or width * height > MAX_PIXELS:
                raise ValueError("decoder_frame_dimensions_exceeded")
            first = False
        chunks.extend((header, payload))
        if kind == b"IEND":
            if length:
                raise ValueError("invalid_decoder_png_end")
            return b"".join(chunks)


def stop_process(process):
    if process is not None and process.poll() is None:
        try:
            process.terminate()
        except ProcessLookupError:
            return
        try:
            process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            try:
                process.kill()
            except ProcessLookupError:
                return
            process.wait(timeout=3)


class LiveReader:
    """One bounded latest-frame slot; inference cannot build an ever-growing backlog."""

    def __init__(self, origin, ffmpeg, fps, *, open_timeout=60, stall_timeout=30):
        self.origin, self.ffmpeg, self.fps = origin, ffmpeg, fps
        self.open_timeout, self.stall_timeout = open_timeout, stall_timeout
        self.stopped = threading.Event()
        self.condition = threading.Condition()
        self.latest = None
        self.notices = deque(maxlen=64)
        self.process = None
        self.last_activity = 0.0
        self.received = self.dropped = self.sessions = 0
        self.thread = threading.Thread(target=self._run, name="intercom-media-reader", daemon=True)

    def notice(self, kind, **data):
        with self.condition:
            self.notices.append({"type": kind, "at": utc_now(), **data})
            self.condition.notify_all()

    def start(self):
        self.thread.start()

    def drain_notices(self):
        with self.condition:
            notices = list(self.notices)
            self.notices.clear()
            return notices

    def take(self, timeout=.5):
        with self.condition:
            if self.latest is None:
                self.condition.wait(timeout)
            packet, self.latest = self.latest, None
            stalled = self.process is not None and time.monotonic() - self.last_activity > self.stall_timeout
            process = self.process if stalled else None
        if stalled:
            self.notice("stream_stalled")
            stop_process(process)
        return packet

    def close(self):
        self.stopped.set()
        with self.condition:
            process = self.process
            self.condition.notify_all()
        stop_process(process)
        if self.thread.ident is not None:
            self.thread.join(timeout=4)
        # A pending HTTP resolve is bounded by open_timeout; it checks stopped before spawning.

    def _consume(self, url):
        command = [self.ffmpeg, "-hide_banner", "-loglevel", "quiet", "-nostdin",
                   "-rw_timeout", str(int(self.stall_timeout * 1_000_000)),
                   "-protocol_whitelist", "http,https,tcp,tls", "-live_start_index", "-1",
                   "-threads", "1", "-re", "-i", url, "-map", "0:v:0", "-an", "-sn", "-dn",
                   "-vf", f"fps={self.fps:g}", "-c:v", "png", "-compression_level", "1",
                   "-threads", "1", "-f", "image2pipe", "pipe:1"]
        # Inherit the collector's process group so its supervisor can also reap FFmpeg on a forced stop.
        process = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                                   stderr=subprocess.DEVNULL)
        with self.condition:
            self.process = process
            self.last_activity = time.monotonic()
            self.sessions += 1
        try:
            first, sequence = True, 0
            while not self.stopped.is_set():
                png = read_png(process.stdout)
                sequence += 1
                now = time.monotonic()
                packet = {"png": png, "receivedAt": utc_now(), "receivedMonotonic": now,
                          "streamEpoch": self.sessions, "decoderFrameIndex": sequence}
                with self.condition:
                    self.received += 1
                    self.dropped += int(self.latest is not None)
                    self.latest = packet
                    self.last_activity = now
                    self.condition.notify_all()
                if first:
                    self.notice("stream_connected", streamEpoch=self.sessions)
                    first = False
        finally:
            stop_process(process)
            process.stdout.close()
            with self.condition:
                self.process = None
            self.notice("stream_disconnected", decoderExitCode=process.returncode)

    def _run(self):
        backoff = 2
        while not self.stopped.is_set():
            before, reason = self.received, None
            try:
                self.notice("stream_connecting")
                url = resolve_playlist(self.origin, self.open_timeout)
                if self.stopped.is_set():
                    break
                self._consume(url)
            except (ValueError, EOFError) as error:
                reason = str(error)
            except (OSError, subprocess.SubprocessError):
                reason = "decoder_process_error"
            except Exception as error:
                # Keep the collector alive and avoid printing remote bodies, URLs or credentials.
                reason = f"reader_error_{type(error).__name__}"
            if self.received > before:
                backoff = 2
            if reason and not self.stopped.is_set():
                self.notice("stream_retry", reason=reason, retryInSeconds=backoff)
            if self.stopped.wait(backoff):
                break
            backoff = min(30, backoff * 2)
