"""Private, size-bounded local debug capture; never deletes or overwrites old data."""

from datetime import datetime, timezone
import json
import shutil


class StorageLimit(Exception):
    pass


def json_bytes(value):
    return (json.dumps(value, ensure_ascii=False, allow_nan=False) + "\n").encode("utf-8")


class CaptureStore:
    def __init__(self, directory, *, max_bytes, min_free_bytes):
        self.directory, self.max_bytes, self.min_free_bytes = directory, max_bytes, min_free_bytes
        directory.mkdir(parents=True, mode=0o700, exist_ok=False)
        (directory / "frames").mkdir(mode=0o700)
        self.journal = (directory / "events.jsonl").open("xb", buffering=0)
        self.bytes_written = self.snapshots = self.events = 0

    def check_space(self, count):
        if self.bytes_written + count + 65536 > self.max_bytes:
            raise StorageLimit("capture_size_limit")
        if shutil.disk_usage(self.directory).free - count < self.min_free_bytes:
            raise StorageLimit("minimum_free_space_limit")

    def write_new(self, relative, payload):
        with (self.directory / relative).open("xb") as stream:
            stream.write(payload)
        self.bytes_written += len(payload)

    def manifest(self, value):
        payload = json_bytes(value)
        self.check_space(len(payload))
        self.write_new("manifest.json", payload)

    def event(self, value):
        payload = json_bytes(value)
        self.check_space(len(payload))
        self.journal.write(payload)
        self.bytes_written += len(payload)
        self.events += 1

    def snapshot(self, image, event):
        import cv2

        # Lossless, native resolution: debug images retain the pixels actually analysed.
        ok, encoded = cv2.imencode(".webp", image, [cv2.IMWRITE_WEBP_QUALITY, 101])
        if not ok:
            raise OSError("Cannot encode a lossless WebP screenshot")
        timestamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S.%fZ")
        stem = f"frames/{timestamp}-{self.snapshots + 1:06d}"
        event["screenshot"] = {"file": stem + ".webp", "metadata": stem + ".json",
                               "width": image.shape[1], "height": image.shape[0],
                               "bytes": int(encoded.size), "format": "webp", "lossless": True}
        metadata = json_bytes(event)
        self.check_space(int(encoded.size) + 2 * len(metadata))
        self.write_new(stem + ".webp", encoded.tobytes())
        self.write_new(stem + ".json", metadata)
        self.snapshots += 1

    def close(self, summary):
        self.journal.close()
        self.write_new("summary.json", json_bytes({**summary, "snapshots": self.snapshots,
                                                  "events": self.events, "bytesBeforeSummary": self.bytes_written}))
