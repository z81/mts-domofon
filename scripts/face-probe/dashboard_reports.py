"""Bounded, read-only views of reports created by the debug dashboard."""

import json
import re


RUN_ID = re.compile(r"session-\d{8}T\d{6}\.\d{6}Z\Z")
FRAME_FILE = re.compile(r"\d{8}T\d{6}\.\d{6}Z-\d{6,}\.(webp|json)\Z")


def read_json(path, limit=512_000):
    try:
        if path.is_symlink():
            return None
        with path.open("rb") as source:
            payload = source.read(limit + 1)
        if len(payload) > limit:
            return None
        return json.loads(payload)
    except (OSError, ValueError):
        return None


def tail_lines(path, *, limit=256_000, count=200):
    try:
        if path.is_symlink():
            return []
        with path.open("rb") as source:
            size = source.seek(0, 2)
            source.seek(max(0, size - limit))
            data = source.read(limit)
        lines = data.splitlines(keepends=True)
        if size > limit:
            lines = lines[1:]
        # The worker may still be appending the final line.
        return [line.decode("utf-8", errors="replace").rstrip() for line in lines[-count:]
                if line.endswith(b"\n")]
    except OSError:
        return []


class Reports:
    def __init__(self, root):
        self.root = root.resolve()
        self.last_screenshots = {}

    def directory(self, run_id):
        if not RUN_ID.fullmatch(run_id):
            raise ValueError("Invalid run ID")
        directory = self.root / run_id
        if directory.is_symlink() or not directory.is_dir():
            raise FileNotFoundError("Run not found")
        return directory

    def runs(self):
        names = sorted((p.name for p in self.root.iterdir()
                        if RUN_ID.fullmatch(p.name) and p.is_dir() and not p.is_symlink()), reverse=True)
        return names[:50]

    def asset(self, run_id, relative):
        directory = self.directory(run_id)
        allowed = relative in ("service.json", "worker.log", "capture/manifest.json",
                               "capture/summary.json", "capture/events.jsonl")
        if relative.startswith("capture/frames/"):
            allowed = bool(FRAME_FILE.fullmatch(relative.removeprefix("capture/frames/")))
        if not allowed:
            raise FileNotFoundError("Report file not found")
        path = (directory / relative).resolve(strict=True)
        if not path.is_relative_to(directory) or not path.is_file():
            raise FileNotFoundError("Report file not found")
        return path

    def detail(self, run_id):
        directory = self.directory(run_id)
        capture = directory / "capture"
        # Never follow a substituted report tree outside the mounted data directory.
        if capture.is_symlink():
            raise ValueError("Symlinked capture directories are not supported")
        events = []
        for line in tail_lines(capture / "events.jsonl"):
            try:
                value = json.loads(line)
                if isinstance(value, dict):
                    events.append(value)
            except ValueError:
                continue
        saved = next((event for event in reversed(events) if event.get("screenshot")), None)
        if saved is None:
            saved = self.last_screenshots.get(run_id)
        if saved is None and run_id not in self.last_screenshots:
            frames = capture / "frames"
            if frames.is_dir() and not frames.is_symlink():
                latest = max((p.name for p in frames.iterdir()
                              if p.suffix == ".json" and FRAME_FILE.fullmatch(p.name)), default=None)
                if latest:
                    saved = read_json(frames / latest)
        self.last_screenshots[run_id] = saved
        heartbeat = next((event for event in reversed(events) if event.get("type") == "heartbeat"), {})
        summary = read_json(capture / "summary.json")
        stats = dict(summary or heartbeat)
        stats["analyzedFrames"] = max([stats.get("analyzedFrames", 0)] +
                                      [event.get("analysisIndex", 0) for event in events])
        if saved and saved.get("screenshot"):
            name = saved["screenshot"]["file"].rsplit("/", 1)[-1]
            if FRAME_FILE.fullmatch(name):
                stats["savedScreenshots"] = max(stats.get("savedScreenshots", 0),
                                                int(name.rsplit("-", 1)[-1].split(".", 1)[0]))
        return {"id": run_id, "service": read_json(directory / "service.json") or {},
                "manifest": read_json(capture / "manifest.json"), "summary": summary,
                "stats": stats, "events": events, "screenshot": saved,
                "logs": tail_lines(directory / "worker.log", limit=64_000, count=150)}


def write_state(directory, value):
    temporary = directory / "service.pending.json"
    with temporary.open("w", encoding="utf-8") as output:
        json.dump(value, output, ensure_ascii=False, allow_nan=False)
        output.write("\n")
    temporary.replace(directory / "service.json")
