"""Own exactly one collector child; no automatic capture on service startup."""

from collections import deque
from datetime import datetime, timezone
import fcntl
import math
import os
from pathlib import Path
import signal
import subprocess
import sys
import threading

from assets import verify_models
from dashboard_reports import Reports, write_state
from reference_inputs import reference_files
from watch_stream import utc_now


HERE = Path(__file__).resolve().parent
LOG_LIMIT = 1_000_000


def capture_options(data):
    if not isinstance(data, dict) or set(data) - {"mode", "fps", "snapshotInterval", "duration"}:
        raise ValueError("Unsupported capture settings")
    mode = data.get("mode", "compare")
    if mode not in ("compare", "detect_only"):
        raise ValueError("Choose compare or detect_only")
    values = {"fps": data.get("fps", 3), "snapshotInterval": data.get("snapshotInterval", 5),
              "duration": data.get("duration", 0)}
    for name, value in values.items():
        if isinstance(value, bool) or not isinstance(value, (int, float)) or not math.isfinite(value):
            raise ValueError(f"{name} must be a finite number")
    if not 0 < values["fps"] <= 10 or not 0 <= values["snapshotInterval"] <= 3600 or not 0 <= values["duration"] <= 604800:
        raise ValueError("FPS: (0, 10]; screenshot interval: 0..3600 s; duration: 0..604800 s")
    return {"mode": mode, **values}


class Collector:
    def __init__(self, args):
        self.args = args
        args.data_dir.mkdir(parents=True, mode=0o700, exist_ok=True)
        self.owner_lock = (args.data_dir / ".dashboard.lock").open("a")
        try:
            fcntl.flock(self.owner_lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            self.owner_lock.close()
            raise ValueError("Another dashboard already owns this data directory") from None
        self.reports = Reports(args.data_dir)
        self.lock = threading.RLock()
        self.process = self.run_id = self.monitor = None
        self.stopping = self.closing = False
        self.record = {}
        self.logs = deque(maxlen=150)

    def reference_status(self):
        try:
            count = len(reference_files(self.args.references)) if self.args.references else 0
            return {"count": count, "error": None if count else "Эталоны не подключены"}
        except ValueError as error:
            return {"count": 0, "error": str(error)}

    def start(self, data):
        options = capture_options(data)
        with self.lock:
            if self.closing or self.process is not None:
                raise RuntimeError("Сбор уже запущен или завершается")
            verify_models(self.args.model_root, [] if options["mode"] == "detect_only" else ["facelivtv2_l"])
            if options["mode"] == "compare":
                status = self.reference_status()
                if not status["count"]:
                    raise ValueError(status["error"])
            run_id = "session-" + datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S.%fZ")
            directory = self.args.data_dir / run_id
            directory.mkdir(mode=0o700)
            command = [sys.executable, "-u", str(HERE / "watch.py"),
                       "--base-url", self.args.base_url, "--model-root", str(self.args.model_root),
                       "--output", str(directory / "capture"), "--fps", str(options["fps"]),
                       "--snapshot-interval", str(options["snapshotInterval"]),
                       "--heartbeat-interval", "5",
                       "--duration", str(options["duration"]), "--max-mb", str(self.args.max_mb),
                       "--min-free-mb", str(self.args.min_free_mb), "--threads", str(self.args.threads)]
            command += ["--detect-only"] if options["mode"] == "detect_only" else ["--references", str(self.args.references)]
            self.record = {"startedAt": utc_now(), "state": "running", "options": options,
                           "doorActionsEnabled": False, "logTruncated": False}
            write_state(directory, self.record)
            try:
                process = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                                           stderr=subprocess.STDOUT, start_new_session=True)
            except OSError:
                self.record.update(state="failed", finishedAt=utc_now(), error="worker_start_failed")
                write_state(directory, self.record)
                raise
            self.process, self.run_id, self.stopping = process, run_id, False
            self.logs.clear()
            self.monitor = threading.Thread(target=self._monitor, args=(process, directory), daemon=True)
            self.monitor.start()
            return run_id

    def _monitor(self, process, directory):
        written = 0
        try:
            with (directory / "worker.log").open("xb", buffering=0) as log:
                while line := process.stdout.readline(8192):
                    text = line.decode("utf-8", errors="replace").rstrip()
                    with self.lock:
                        self.logs.append(text)
                    if written + len(line) <= LOG_LIMIT:
                        log.write(line)
                        written += len(line)
                    else:
                        with self.lock:
                            self.record["logTruncated"] = True
            code = process.wait()
        except OSError:
            with self.lock:
                self.record["error"] = "worker_log_io_error"
            self._terminate(process)
            code = process.wait()
        finally:
            process.stdout.close()
        # FFmpeg shares this worker's dedicated process group, never the dashboard's group.
        self._kill_group(process)
        with self.lock:
            self.record.update(state="stopped" if code == 0 or self.stopping else "failed",
                               finishedAt=utc_now(), exitCode=code)
            try:
                write_state(directory, self.record)
            finally:
                self.process, self.stopping = None, False

    @staticmethod
    def _kill_group(process):
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass

    @classmethod
    def _terminate(cls, process):
        try:
            process.send_signal(signal.SIGINT)
            process.wait(timeout=15)
        except subprocess.TimeoutExpired:
            cls._kill_group(process)
            process.wait(timeout=5)
        except ProcessLookupError:
            pass

    def stop(self):
        with self.lock:
            if self.process is None or self.stopping:
                return
            self.stopping = True
            threading.Thread(target=self._terminate, args=(self.process,), daemon=True).start()

    def state(self, selected=None):
        with self.lock:
            runs = self.reports.runs()
            run_id = selected or self.run_id or (runs[0] if runs else None)
            detail = self.reports.detail(run_id) if run_id else None
            active = self.run_id if self.process else None
            if detail:
                if run_id == self.run_id:
                    detail["logs"] = list(self.logs)
                    detail["service"] = dict(self.record)
                if run_id == active:
                    detail["service"]["state"] = "stopping" if self.stopping else "running"
                elif detail["service"].get("state") == "running":
                    detail["service"]["state"] = "interrupted"
            return {"activeRun": active, "activeOptions": self.record.get("options") if active else None,
                    "stopping": self.stopping, "runs": runs, "run": detail,
                    "config": {"baseUrl": self.args.base_url, "model": "FaceLiVTv2-L", "detector": "SCRFD",
                               "references": self.reference_status(), "maxMb": self.args.max_mb,
                               "minFreeMb": self.args.min_free_mb, "threads": self.args.threads,
                               "doorActionsEnabled": False}}

    def close(self):
        with self.lock:
            self.closing = True
            process, monitor = self.process, self.monitor
            self.stopping = process is not None
        if process:
            self._terminate(process)
        if monitor:
            monitor.join(timeout=5)
        self.owner_lock.close()
