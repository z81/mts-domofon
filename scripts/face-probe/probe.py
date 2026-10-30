#!/usr/bin/env python3
"""Offline video probe. Accepts local files only; never opens a camera or door."""

import argparse
from datetime import datetime, timezone
import json
import math
import os
from pathlib import Path
import resource
import statistics
import sys
import time


HERE = Path(__file__).resolve().parent
VIDEO_SUFFIXES = {".mp4", ".m4v", ".mov", ".mkv", ".avi", ".webm"}


def arguments(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("video", type=Path, help="A local video, not a URL or camera index")
    parser.add_argument("--references", type=Path, help="An image, a flat photo folder, or folders per person")
    parser.add_argument("--models", type=Path, default=HERE / "models" / "buffalo_l")
    parser.add_argument("--recognizer", type=Path, help="Override w600k_r50.onnx with an ArcFace-compatible ONNX")
    parser.add_argument("--output", type=Path, help="New report directory; must not already exist")
    sampling = parser.add_mutually_exclusive_group()
    sampling.add_argument("--sample-fps", type=float, default=3)
    sampling.add_argument("--all-frames", action="store_true", help="Process every decoded frame in the time range")
    parser.add_argument("--det-size", type=int, default=960)
    parser.add_argument("--reference-det-size", type=int, default=640,
                        help="Independent detector size for reference portraits")
    parser.add_argument("--threads", type=int, default=3)
    parser.add_argument("--min-face", type=float, default=60, help="Minimum face short side in source pixels")
    parser.add_argument("--min-eyes", type=float, default=20, help="Minimum eye separation in source pixels")
    parser.add_argument("--start", type=float, default=0, help="Start time in seconds, inclusive")
    parser.add_argument("--end", type=float, help="End time in seconds, exclusive")
    args = parser.parse_args(argv)
    args.video = args.video.expanduser().resolve()
    if not args.video.is_file() or args.video.suffix.lower() not in VIDEO_SUFFIXES:
        parser.error("video must be an existing local MP4, MOV, MKV, AVI, M4V or WebM file")
    if not math.isfinite(args.sample_fps) or not 0 < args.sample_fps <= 60:
        parser.error("--sample-fps must be finite and in (0, 60]")
    for name in ("det_size", "reference_det_size"):
        value = getattr(args, name)
        if not 320 <= value <= 1920 or value % 32:
            parser.error(f"--{name.replace('_', '-')} must be a multiple of 32 between 320 and 1920")
    if not 1 <= args.threads <= 64:
        parser.error("--threads must be between 1 and 64")
    if any(not math.isfinite(value) or value <= 0 for value in (args.min_face, args.min_eyes)):
        parser.error("--min-face and --min-eyes must be finite and positive")
    if not math.isfinite(args.start) or args.start < 0:
        parser.error("--start must be finite and nonnegative")
    if args.end is not None and (not math.isfinite(args.end) or args.end <= args.start):
        parser.error("--end must be finite and greater than --start")
    args.models = args.models.expanduser().resolve()
    args.detector = args.models / "det_10g.onnx"
    args.recognizer = (args.recognizer or args.models / "w600k_r50.onnx").expanduser().resolve()
    needed_models = [args.detector] + ([args.recognizer] if args.references else [])
    if any(not path.is_file() or path.suffix.lower() != ".onnx" for path in needed_models):
        parser.error("Required local ONNX models are missing; see scripts/face-probe/README.md")
    if args.references:
        args.references = args.references.expanduser().resolve()
        if not args.references.exists():
            parser.error("Reference path does not exist")
    timestamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S.%fZ")
    args.output = (args.output or HERE / "reports" / timestamp).expanduser().absolute()
    if args.output.exists() or args.output.is_symlink():
        parser.error("Output directory already exists; choose a new one to preserve previous reports")
    return args


def write_json(path, data):
    pending = path.with_suffix(".partial.json")
    with pending.open("x", encoding="utf-8") as stream:
        json.dump(data, stream, ensure_ascii=False, indent=2, allow_nan=False)
        stream.write("\n")
    pending.rename(path)


def analyze_video(args, engine):
    import cv2

    # Limit FFmpeg inputs to local files, including nested container resources.
    os.environ["OPENCV_FFMPEG_CAPTURE_OPTIONS"] = "protocol_whitelist;file"
    capture = cv2.VideoCapture(str(args.video), cv2.CAP_FFMPEG)
    try:
        if not capture.isOpened():
            raise ValueError("Cannot open video with the FFmpeg backend")
        fps = capture.get(cv2.CAP_PROP_FPS)
        if not math.isfinite(fps) or fps <= 0:
            raise ValueError("Video has no usable frame rate for timestamp fallback")
        width = int(capture.get(cv2.CAP_PROP_FRAME_WIDTH))
        height = int(capture.get(cv2.CAP_PROP_FRAME_HEIGHT))
        decoded, sampled, with_faces, detected, eligible = 0, 0, 0, 0, 0
        inference_ms, best_scores = [], {}
        last_time, next_sample = -1.0, args.start
        with (args.output / "frames.jsonl").open("x", encoding="utf-8") as rows:
            while True:
                ok, frame = capture.read()
                if not ok:
                    break
                raw_time = capture.get(cv2.CAP_PROP_POS_MSEC) / 1000
                if math.isfinite(raw_time) and raw_time > last_time:
                    timestamp = raw_time
                else:
                    timestamp = max(decoded / fps, last_time + 1 / fps)
                frame_index = decoded
                decoded += 1
                last_time = timestamp
                if args.end is not None and timestamp >= args.end:
                    break
                if timestamp + 1e-6 < args.start:
                    continue
                if not args.all_frames:
                    if timestamp + 1e-6 < next_sample:
                        continue
                    sample_slot = math.floor((timestamp - args.start + 1e-6) * args.sample_fps)
                    next_sample = args.start + (sample_slot + 1) / args.sample_fps
                tick = time.perf_counter()
                faces = []
                for box, landmarks in engine.detect(frame):
                    quality = engine.measure(frame, box, landmarks)
                    scores = []
                    if quality["eligible"] and engine.references:
                        scores = engine.compare(frame, box, landmarks)
                    face = {**quality, "comparisons": scores}
                    faces.append(face)
                    eligible += int(quality["eligible"])
                    for score in scores:
                        previous = best_scores.get(score["label"])
                        if previous is None or score["maxCosine"] > previous["maxCosine"]:
                            best_scores[score["label"]] = {
                                **score, "timeSec": round(timestamp, 3),
                                "frameIndex": frame_index, "faceIndex": len(faces) - 1,
                            }
                elapsed = (time.perf_counter() - tick) * 1000
                inference_ms.append(elapsed)
                sampled += 1
                with_faces += bool(faces)
                detected += len(faces)
                row = {"timeSec": round(timestamp, 3), "frameIndex": frame_index,
                       "inferenceMs": round(elapsed, 2), "faces": faces}
                rows.write(json.dumps(row, ensure_ascii=False, allow_nan=False) + "\n")
                if sampled % 30 == 0:
                    print(f"{timestamp:.1f}s: {sampled} samples, {eligible} eligible faces", file=sys.stderr, flush=True)
        if not sampled:
            raise ValueError("No frames decoded in the selected time range")
        return {
            "videoFile": args.video.name, "width": width, "height": height,
            "sourceFps": round(fps, 3), "decodedFrames": decoded,
            "sampledFrames": sampled, "framesWithFaces": with_faces,
            "detectedFaces": detected, "eligibleFaces": eligible,
            "inferenceMedianMs": round(statistics.median(inference_ms), 2),
            "bestScoresByLabel": best_scores,
        }
    finally:
        capture.release()


def main():
    args = arguments()
    # --help and invalid arguments work without installing ML dependencies.
    from engine import FaceEngine

    # New outputs are private on POSIX, including incomplete interrupted runs.
    os.umask(0o077)
    started = time.perf_counter()
    engine = FaceEngine(args.detector, args.recognizer if args.references else None,
                        det_size=args.det_size, threads=args.threads,
                        min_face=args.min_face, min_eyes=args.min_eyes)
    references = engine.load_references(args.references, det_size=args.reference_det_size) if args.references else []
    args.output.mkdir(parents=True, mode=0o700, exist_ok=False)
    print(f"Report directory: {args.output}", file=sys.stderr, flush=True)
    summary = analyze_video(args, engine)
    peak_rss = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
    summary.update({
        "status": "complete", "mode": "comparison" if references else "detection_only",
        "models": engine.models, "references": references,
        "settings": {key: getattr(args, key) for key in
                     ("sample_fps", "all_frames", "det_size", "reference_det_size", "threads", "min_face", "min_eyes", "start", "end")},
        "wallSeconds": round(time.perf_counter() - started, 2),
        "peakProcessRamMb": round(peak_rss * (1 if sys.platform == "darwin" else 1024) / 1_000_000, 1),
        "scoreMeaning": "Cosine similarities, not probabilities or calibrated identity decisions. No tracking or liveness.",
    })
    write_json(args.output / "summary.json", summary)
    print(json.dumps(summary, ensure_ascii=False, indent=2, allow_nan=False))


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        print("Interrupted; a report without summary.json is incomplete.", file=sys.stderr)
        sys.exit(130)
    except (OSError, ValueError, ImportError, RuntimeError) as error:
        print(f"Error: {error}\nSee scripts/face-probe/README.md. No completed report was produced.", file=sys.stderr)
        sys.exit(1)
