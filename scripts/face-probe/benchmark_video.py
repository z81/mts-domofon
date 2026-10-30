"""Profile one local clip without reference photos, identity matching or saved embeddings."""

import hashlib
import json
import math
import os
import resource
import sys
import time

from assets import MODEL_FILES, sha256


MODES = ("onnx", "faceEmbedding", "detection", "decode", "frame")
NOTE = (
    "One local clip, every decoded frame, CPU only. No references, similarity scores, "
    "identity decisions or saved embeddings. ONNX times only recognizer session.run; "
    "faceEmbedding also includes alignment, preprocessing and L2 normalization. "
    "Detection/decode/frame timings are per frame; recognizer timings are per eligible face. "
    "Loading and synthetic warmup are excluded from latency. Background load can affect results."
)


class TimedSession:
    def __init__(self, session, samples):
        self.session, self.samples = session, samples

    def run(self, *args, **kwargs):
        started = time.perf_counter_ns()
        result = self.session.run(*args, **kwargs)
        self.samples.append((time.perf_counter_ns() - started) / 1_000_000)
        return result


def benchmark_video(args, stats):
    import cv2
    import numpy as np
    import onnxruntime as ort
    from engine import FaceEngine

    started = time.perf_counter()
    engine = FaceEngine(args.model_root / "buffalo_l/det_10g.onnx",
                        args.model_root / MODEL_FILES[args.worker_model],
                        det_size=640, threads=args.threads, min_face=60, min_eyes=20)
    load_seconds = time.perf_counter() - started
    if engine.references:
        raise ValueError("Speed benchmark must not load any reference identities")
    for model in (engine.detector, engine.recognizer):
        if model.session.get_providers() != ["CPUExecutionProvider"]:
            raise ValueError("Benchmark must use only CPUExecutionProvider")
    video_hash = sha256(args.video)
    os.environ["OPENCV_FFMPEG_CAPTURE_OPTIONS"] = "protocol_whitelist;file"
    capture = cv2.VideoCapture(str(args.video), cv2.CAP_FFMPEG)
    samples = {mode: [] for mode in MODES}
    signature = hashlib.sha256(video_hash.encode())
    detected, eligible = 0, 0
    try:
        if not capture.isOpened():
            raise ValueError("Cannot open local video with the FFmpeg backend")
        width, height = (int(capture.get(prop)) for prop in (cv2.CAP_PROP_FRAME_WIDTH, cv2.CAP_PROP_FRAME_HEIGHT))
        if width <= 0 or height <= 0:
            raise ValueError("Invalid video dimensions")
        fps = capture.get(cv2.CAP_PROP_FPS)
        fps = fps if math.isfinite(fps) and fps > 0 else None
        artificial_frame = np.zeros((height, width, 3), dtype=np.uint8)
        artificial_crop = np.random.default_rng(args.seed).integers(0, 256, (112, 112, 3), dtype=np.uint8)
        print(f"Synthetic warmup: {args.warmup} detector/recognizer calls", flush=True)
        for _ in range(args.warmup):
            engine.detect(artificial_frame)
            engine.recognizer.get_feat(artificial_crop)
        del artificial_frame, artificial_crop
        engine.recognizer.session = TimedSession(engine.recognizer.session, samples["onnx"])
        load_before = os.getloadavg()
        started = time.perf_counter()
        while True:
            frame_started = time.perf_counter_ns()
            ok, frame = capture.read()
            decode_ms = (time.perf_counter_ns() - frame_started) / 1_000_000
            if not ok:
                break
            samples["decode"].append(decode_ms)
            tick = time.perf_counter_ns()
            faces = engine.detect(frame)
            samples["detection"].append((time.perf_counter_ns() - tick) / 1_000_000)
            detected += len(faces)
            detections = []
            for box, landmarks in faces:
                quality = engine.measure(frame, box, landmarks)
                detections.append({"quality": quality, "landmarks": landmarks.tolist()})
                if quality["eligible"]:
                    tick = time.perf_counter_ns()
                    vector = engine.embed(frame, box, landmarks)
                    samples["faceEmbedding"].append((time.perf_counter_ns() - tick) / 1_000_000)
                    del vector  # Deliberately never store or compare feature vectors.
                    eligible += 1
            samples["frame"].append((time.perf_counter_ns() - frame_started) / 1_000_000)
            signature.update(json.dumps(detections, sort_keys=True, allow_nan=False).encode())
            signature.update(b"\n")
            if len(samples["frame"]) % 100 == 0:
                print(f"Decoded {len(samples['frame'])} frames; {eligible} eligible faces", flush=True)
        wall_seconds = time.perf_counter() - started
        if not samples["frame"]:
            raise ValueError("No frames decoded")
    finally:
        capture.release()
    peak_rss = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
    return {
        "model": args.worker_model, "status": "complete", "syntheticOnly": False,
        "mode": "video_timing_only", "videoFile": args.video.name, "videoSha256": video_hash,
        "samplingSignature": signature.hexdigest(), "sourceFps": fps, "width": width, "height": height,
        "decodedFrames": len(samples["frame"]), "detectedFaces": detected, "eligibleFaces": eligible,
        "processedFps": len(samples["frame"]) / wall_seconds, "videoWallSeconds": wall_seconds,
        "models": engine.models, "loadSeconds": load_seconds, "onnxruntime": ort.__version__,
        "loadAverageBefore": load_before, "loadAverageAfter": os.getloadavg(),
        "peakProcessRamMb": peak_rss * (1 if sys.platform == "darwin" else 1024) / 1_000_000,
        "timings": {mode: stats(values) if values else None for mode, values in samples.items()},
        "samplesMs": samples,
    }


def markdown(report, cell):
    lines = [f"# Single-clip speed ({report['status']})", "", NOTE, "",
             f"CPU: {cell(report['environment']['cpu'])}; threads: {report['settings']['intraOpThreads']}; "
             f"clip: {cell(report['videoFile'])}.", "",
             "| Model | ONNX median, ms/face | ONNX p95, ms/face | Face total median, ms | Detector median, ms/frame | Video, s | Video FPS | Peak RAM, MB |",
             "|---|---:|---:|---:|---:|---:|---:|---:|"]
    completed = sorted((row for row in report["results"] if row["status"] == "complete"),
                       key=lambda row: row["timings"]["onnx"]["medianMs"] if row["timings"]["onnx"] else math.inf)
    for row in completed:
        raw, face = row["timings"]["onnx"], row["timings"]["faceEmbedding"]
        values = [row["model"], f"{raw['medianMs']:.3f}" if raw else "—",
                  f"{raw['p95Ms']:.3f}" if raw else "—", f"{face['medianMs']:.3f}" if face else "—",
                  f"{row['timings']['detection']['medianMs']:.3f}", f"{row['videoWallSeconds']:.2f}",
                  f"{row['processedFps']:.2f}", f"{row['peakProcessRamMb']:.1f}"]
        lines.append("| " + " | ".join(map(cell, values)) + " |")
    lines.extend(["", f"Same video/detections/runtime: {report['comparableInputsAndRuntime']}.", ""])
    for row in report["results"]:
        if row["status"] == "complete":
            lines.append(f"- {cell(row['model'])}: {row['decodedFrames']} frames, "
                         f"{row['eligibleFaces']} eligible faces. No eligible faces means no recognizer timing.")
        else:
            lines.append(f"- {cell(row['model'])}: failed; {cell(row['error'])}")
    lines.extend(["", "Video seconds/FPS exclude loading and warmup; JSON processSeconds includes them. "
                  "No reference photos or similarity/identity results are produced."])
    return "\n".join(lines) + "\n"
