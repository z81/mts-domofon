#!/usr/bin/env python3
"""Observe the intercom stream and save local debug captures. Never opens a door."""

import argparse
from datetime import datetime, timezone
import math
import os
from pathlib import Path
import shutil
import signal
import sys
import time

from assets import MODEL_FILES, ROOT, verify_models
from probe import HERE
from reference_inputs import reference_files
from watch_store import CaptureStore, StorageLimit
from watch_stream import LiveReader, base_origin, utc_now


SCORE_MEANING = (
    "detectionScore is the detector score, not identity confidence. maxCosine/meanCosine are "
    "uncalibrated similarities, not probabilities. No identity decisions, tracking, liveness or door control."
)


def arguments(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base-url", default="http://127.0.0.1:3000")
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--references", type=Path, help="Local reference image/album for FaceLiVTv2-L cosine scores")
    mode.add_argument("--detect-only", action="store_true", help="Only SCRFD detection; no identity embeddings/scores")
    parser.add_argument("--model-root", type=Path, default=ROOT)
    parser.add_argument("--output", type=Path, help="New private report directory")
    parser.add_argument("--fps", type=float, default=3)
    parser.add_argument("--snapshot-interval", type=float, default=5)
    parser.add_argument("--heartbeat-interval", type=float, default=30, help="Status event interval in seconds (>= 1)")
    parser.add_argument("--duration", type=float, default=0, help="Seconds; 0 runs until Ctrl+C or a storage limit")
    parser.add_argument("--max-mb", type=float, default=1024, help="Maximum new capture bytes in decimal MB; no deletion")
    parser.add_argument("--min-free-mb", type=float, default=256)
    parser.add_argument("--threads", type=int, default=3)
    parser.add_argument("--det-size", type=int, default=640)
    parser.add_argument("--reference-det-size", type=int, default=640)
    parser.add_argument("--min-face", type=float, default=60)
    parser.add_argument("--min-eyes", type=float, default=20)
    args = parser.parse_args(argv)
    args.base_url = base_origin(args.base_url)
    if not math.isfinite(args.fps) or not 0 < args.fps <= 10:
        parser.error("--fps must be finite and in (0, 10]")
    for name in ("snapshot_interval", "duration", "min_free_mb"):
        if not math.isfinite(getattr(args, name)) or getattr(args, name) < 0:
            parser.error(f"--{name.replace('_', '-')} must be finite and nonnegative")
    for name in ("max_mb", "min_face", "min_eyes"):
        if not math.isfinite(getattr(args, name)) or getattr(args, name) <= 0:
            parser.error(f"--{name.replace('_', '-')} must be finite and positive")
    if args.max_mb < 1 or not 1 <= args.threads <= 64:
        parser.error("Need --max-mb >= 1 and --threads in 1..64")
    if not math.isfinite(args.heartbeat_interval) or args.heartbeat_interval < 1:
        parser.error("--heartbeat-interval must be finite and >= 1")
    for name in ("det_size", "reference_det_size"):
        value = getattr(args, name)
        if not 320 <= value <= 1920 or value % 32:
            parser.error(f"--{name.replace('_', '-')} must be a multiple of 32 in 320..1920")
    args.model_root = args.model_root.expanduser().resolve()
    verify_models(args.model_root, [] if args.detect_only else ["facelivtv2_l"])
    if args.references:
        args.references = args.references.expanduser().resolve()
        reference_files(args.references)
    args.ffmpeg = shutil.which("ffmpeg")
    if not args.ffmpeg:
        parser.error("ffmpeg is required on PATH")
    timestamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S.%fZ")
    args.output = (args.output or HERE / "reports" / f"watch-{timestamp}").expanduser().absolute()
    if args.output.exists() or args.output.is_symlink():
        parser.error("Output already exists; existing captures are never overwritten")
    return args


def analyze(engine, image, packet, index):
    started = tick = time.perf_counter()
    detections = engine.detect(image)
    detection_ms = (time.perf_counter() - tick) * 1000
    faces = []
    for box, landmarks in detections:
        quality = engine.measure(image, box, landmarks)
        comparisons, comparison_ms = [], 0
        if not quality["eligible"]:
            status = "quality_rejected"
        elif engine.references:
            tick = time.perf_counter()
            comparisons = engine.compare(image, box, landmarks)
            comparison_ms = (time.perf_counter() - tick) * 1000
            status = "scores_available"
        else:
            status = "detected_only"
        faces.append({"faceIndex": len(faces), "status": status, **quality,
                      "comparisons": comparisons, "identityProbability": None,
                      "comparisonMs": round(comparison_ms, 2)})
    return {"type": "faces" if faces else "no_face", "analyzedAt": utc_now(),
            "receivedAt": packet["receivedAt"], "cameraCapturedAt": None,
            "streamEpoch": packet["streamEpoch"], "decoderFrameIndex": packet["decoderFrameIndex"],
            "analysisIndex": index, "width": image.shape[1], "height": image.shape[0],
            "detectionMs": round(detection_ms, 2), "analysisMs": round((time.perf_counter() - started) * 1000, 2),
            "faces": faces, "screenshot": None,
            "scoreMeaning": SCORE_MEANING, "doorActionsEnabled": False}


def collect(args, engine, references, store, reader, stopped):
    started = time.monotonic()
    last_snapshot, last_heartbeat, last_analysis = -math.inf, -math.inf, -math.inf
    counts = {"analyzedFrames": 0, "framesWithFaces": 0, "detectedFaces": 0, "eligibleFaces": 0,
              "skippedStaleFrames": 0, "skippedRateLimit": 0, "invalidFrames": 0}
    status, reason, code = "stopped", "duration", 0
    stage, error_stage = "manifest", None
    try:
        store.manifest({"startedAt": utc_now(), "sourceOrigin": args.base_url,
                        "sourceRoute": "/api/v1/cameras/stream", "models": engine.models, "references": references,
                        "mode": "detection_only" if args.detect_only else "facelivtv2_l_cosine_debug",
                        "settings": {key: getattr(args, key) for key in
                                     ("fps", "snapshot_interval", "heartbeat_interval", "duration", "max_mb", "min_free_mb", "threads",
                                      "det_size", "reference_det_size", "min_face", "min_eyes")},
                        "scoreMeaning": SCORE_MEANING, "doorActionsEnabled": False,
                        "timestampMeaning": "Host receive/analyse times; camera capture time and HLS delay are unknown."})
        stage = "stream_start"
        reader.start()
        while not stopped[0]:
            now = time.monotonic()
            if args.duration and now - started >= args.duration:
                break
            stage = "stream_events"
            for notice in reader.drain_notices():
                store.event(notice)
                print(f"{notice['at']} {notice['type']} {notice.get('reason', '')}", flush=True)
            if now - last_heartbeat >= args.heartbeat_interval:
                store.event({"type": "heartbeat", "at": utc_now(), **counts,
                             "receivedFrames": reader.received, "overwrittenFrames": reader.dropped,
                             "streamEpochs": reader.sessions, "savedScreenshots": store.snapshots})
                last_heartbeat = now
            stage = "stream_read"
            packet = reader.take(timeout=.5)
            if packet is None:
                continue
            now = time.monotonic()
            age_ms = (now - packet["receivedMonotonic"]) * 1000
            if age_ms > 2000:
                counts["skippedStaleFrames"] += 1
                continue
            if now - last_analysis < 1 / args.fps:
                counts["skippedRateLimit"] += 1
                continue
            import cv2
            import numpy as np

            stage = "frame_decode"
            image = cv2.imdecode(np.frombuffer(packet["png"], dtype=np.uint8), cv2.IMREAD_COLOR)
            if image is None:
                counts["invalidFrames"] += 1
                store.event({"type": "decode_error", "at": utc_now()})
                continue
            last_analysis = now
            counts["analyzedFrames"] += 1
            stage = "face_analysis"
            event = analyze(engine, image, packet, counts["analyzedFrames"])
            event["receiveToAnalysisMs"] = round(age_ms, 2)
            if not event["faces"]:
                continue
            counts["framesWithFaces"] += 1
            counts["detectedFaces"] += len(event["faces"])
            counts["eligibleFaces"] += sum(face["eligible"] for face in event["faces"])
            if now - last_snapshot >= args.snapshot_interval:
                stage = "snapshot_write"
                store.snapshot(image, event)
                last_snapshot = now
            else:
                event["snapshotSkipped"] = "cooldown"
            stage = "event_write"
            store.event(event)
            scores = [score["maxCosine"] for face in event["faces"] for score in face["comparisons"]]
            score_text = f"max_cosine={max(scores):.6f}" if scores else "no_comparison_score"
            print(f"{event['analyzedAt']} faces={len(event['faces'])} {score_text} "
                  f"screenshot={event['screenshot']['file'] if event['screenshot'] else 'cooldown'}", flush=True)
        if stopped[0]:
            reason = "signal"
    except StorageLimit as error:
        status, reason, code = "storage_limit", str(error), 2
        error_stage = stage
    except Exception as error:
        # Native OpenCV/ONNX errors need not inherit RuntimeError; never mark them as a normal stop.
        status, reason, code = "error", type(error).__name__, 1
        error_stage = stage
        print(f"Collector failed during {stage}: {reason}; inspect local captures. No door actions were made.",
              file=sys.stderr)
    finally:
        reader.close()
        store.close({"status": status, "stopReason": reason, "errorStage": error_stage,
                     "finishedAt": utc_now(), **counts,
                     "receivedFrames": reader.received, "overwrittenFrames": reader.dropped,
                     "streamEpochs": reader.sessions, "wallSeconds": round(time.monotonic() - started, 2),
                     "doorActionsEnabled": False})
    print(f"Stopped: {status} ({reason}); captures: {args.output}", flush=True)
    return code


def main(argv=None):
    args = arguments(argv)
    from engine import FaceEngine

    os.umask(0o077)
    recognizer = None if args.detect_only else args.model_root / MODEL_FILES["facelivtv2_l"]
    engine = FaceEngine(args.model_root / "buffalo_l/det_10g.onnx", recognizer,
                        det_size=args.det_size, threads=args.threads,
                        min_face=args.min_face, min_eyes=args.min_eyes)
    references = engine.load_references(args.references, det_size=args.reference_det_size) if args.references else []
    store = CaptureStore(args.output, max_bytes=int(args.max_mb * 1_000_000),
                         min_free_bytes=int(args.min_free_mb * 1_000_000))
    reader = LiveReader(args.base_url, args.ffmpeg, args.fps)
    stopped = [False]

    def stop(signum, frame):
        stopped[0] = True

    previous = {sig: signal.signal(sig, stop) for sig in (signal.SIGINT, signal.SIGTERM)}
    print(f"Debug capture only; door control disabled. Output: {args.output}", flush=True)
    try:
        return collect(args, engine, references, store, reader, stopped)
    finally:
        for sig, handler in previous.items():
            signal.signal(sig, handler)


if __name__ == "__main__":
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        sys.exit(130)
    except (OSError, ValueError, ImportError, RuntimeError, StorageLimit) as error:
        print(f"Error: {error}", file=sys.stderr)
        sys.exit(1)
