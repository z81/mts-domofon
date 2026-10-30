#!/usr/bin/env python3
"""CPU speed benchmark: synthetic input or one local video, never identity matching."""

import argparse
from datetime import datetime, timezone
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import resource
import statistics
import subprocess
import sys
import time

from assets import MODEL_FILES, ROOT, sha256, verify_models
from compare import table_cell
from probe import HERE, VIDEO_SUFFIXES, write_json
import benchmark_video


MODES = ("onnx", "alignedEmbedding")
NOTE = (
    "Synthetic uint8 noise, batch=1, 112x112, CPU only. ONNX = prebuilt tensor to output; "
    "alignedEmbedding = already aligned BGR pixels to preprocessed, L2-normalized embedding. "
    "No detection, alignment, decoding, reference comparison or identity/accuracy evaluation. "
    "Warmup/loading are excluded from latency. Rate is serial embeddings/s, not video FPS. "
    "Background applications and thermal state can affect results."
)


def arguments(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model-root", type=Path, default=ROOT)
    parser.add_argument("--model-set", nargs="+", choices=list(MODEL_FILES), help="Default: all installed models")
    source = parser.add_mutually_exclusive_group()
    source.add_argument("--iterations", type=int, default=200, help="Synthetic measured calls per mode and model")
    source.add_argument("--video", type=Path, help="Profile every frame of ONE local clip, without references or matching")
    parser.add_argument("--warmup", type=int, default=20, help="Untimed calls per mode and model")
    parser.add_argument("--threads", type=int, default=3)
    parser.add_argument("--seed", type=int, default=0)
    parser.add_argument("--output", type=Path, help="New report directory")
    parser.add_argument("--worker-model", choices=list(MODEL_FILES), help=argparse.SUPPRESS)
    args = parser.parse_args(argv)
    if args.video:
        args.video = args.video.expanduser().resolve()
        if not args.video.is_file() or args.video.suffix.lower() not in VIDEO_SUFFIXES:
            parser.error("--video must be an existing local video file")
    if not 1 <= args.iterations <= 1_000_000 or not 1 <= args.warmup <= 10_000:
        parser.error("--iterations must be 1..1000000 and --warmup 1..10000")
    if not 1 <= args.threads <= 64 or not 0 <= args.seed <= 2**32 - 1:
        parser.error("--threads must be 1..64 and --seed 0..4294967295")
    if args.worker_model and args.model_set:
        parser.error("Internal worker mode cannot be combined with --model-set")
    args.model_root = args.model_root.expanduser().resolve()
    installed = [name for name, path in MODEL_FILES.items()
                 if (args.model_root / path).exists() or (args.model_root / path).is_symlink()]
    args.unavailable = [name for name in MODEL_FILES if name not in installed]
    args.model_set = [args.worker_model] if args.worker_model else args.model_set or installed
    if not args.model_set or len(set(args.model_set)) != len(args.model_set):
        parser.error("Need at least one installed model and no duplicate model names")
    verify_models(args.model_root, args.model_set)
    timestamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S.%fZ")
    args.output = (args.output or HERE / "reports" / f"speed-{timestamp}").expanduser().absolute()
    if args.output.exists() or args.output.is_symlink():
        parser.error("Output directory already exists; previous reports are never overwritten")
    return args


def latency_stats(samples):
    ordered = sorted(samples)
    mean = statistics.fmean(samples)
    return {
        "count": len(samples), "medianMs": statistics.median(samples), "meanMs": mean,
        "p95Ms": ordered[math.ceil(.95 * len(samples)) - 1],
        "minMs": ordered[0], "maxMs": ordered[-1], "serialPerSecond": 1000 / mean,
    }


def settings(args):
    return {"iterations": None if args.video else args.iterations, "warmup": args.warmup, "seed": args.seed,
            "intraOpThreads": args.threads, "interOpThreads": 1, "opencvThreads": 1,
            "provider": "CPUExecutionProvider", "inputShape": [1, 3, 112, 112],
            "inputPoolSize": None if args.video else 8, "dtype": "float32", "p95Definition": "nearest rank",
            "mode": "video_timing_only" if args.video else "synthetic",
            **({"allFrames": True, "detSize": 640, "minFace": 60, "minEyes": 20} if args.video else {})}


def benchmark_model(args):
    os.environ["ORT_DISABLE_TELEMETRY"] = "1"
    import cv2
    import numpy as np
    import onnxruntime as ort
    from insightface.model_zoo import get_model

    ort.disable_telemetry_events()
    cv2.setNumThreads(1)
    options = ort.SessionOptions()
    options.intra_op_num_threads = args.threads
    options.inter_op_num_threads = 1
    path = args.model_root / MODEL_FILES[args.worker_model]
    weight_hash = sha256(path)
    started = time.perf_counter()
    model = get_model(str(path), download=False, providers=["CPUExecutionProvider"], sess_options=options)
    if getattr(model, "taskname", None) != "recognition":
        raise ValueError("Expected an ArcFace-compatible recognizer")
    model.prepare(ctx_id=-1)
    load_seconds = time.perf_counter() - started
    if tuple(model.input_size) != (112, 112) or model.session.get_inputs()[0].type != "tensor(float)":
        raise ValueError("Expected 112x112 FP32 model input")
    if model.session.get_providers() != ["CPUExecutionProvider"]:
        raise ValueError("Benchmark must use only CPUExecutionProvider")
    images = np.random.default_rng(args.seed).integers(0, 256, (8, 112, 112, 3), dtype=np.uint8)
    blobs = [cv2.dnn.blobFromImages([image], 1 / model.input_std, model.input_size,
                                   (model.input_mean,) * 3, swapRB=True) for image in images]
    feeds = [{model.input_name: blob} for blob in blobs]

    def onnx(index):
        return model.session.run(model.output_names, feeds[index])[0]

    def aligned_embedding(index):
        vector = np.asarray(model.get_feat(images[index]), dtype=np.float32).ravel()
        norm = float(np.linalg.norm(vector))
        if not np.isfinite(vector).all() or not math.isfinite(norm) or norm < 1e-8:
            raise ValueError("Invalid embedding on synthetic input")
        return vector / norm

    operations = {"onnx": onnx, "alignedEmbedding": aligned_embedding}
    # Verify both paths use identical preprocessing; never save feature vectors.
    expected = onnx(0).ravel()
    np.testing.assert_allclose(aligned_embedding(0), expected / np.linalg.norm(expected), rtol=1e-5, atol=1e-6)
    if expected.shape != (512,) or not np.isfinite(expected).all():
        raise ValueError("Expected a finite 512-dimensional output")
    print(f"Warmup: {args.warmup} calls per mode", flush=True)
    for index in range(args.warmup):
        for operation in operations.values():
            operation(index % len(images))
    samples = {mode: [] for mode in MODES}
    load_before = os.getloadavg()
    for index in range(args.iterations):
        # Alternate order to reduce systematic cache/order bias between the modes.
        for mode in MODES if index % 2 == 0 else MODES[::-1]:
            tick = time.perf_counter_ns()
            vector = operations[mode](index % len(images))
            elapsed = (time.perf_counter_ns() - tick) / 1_000_000
            samples[mode].append(elapsed)
            if vector.size != 512 or not np.isfinite(vector).all():
                raise ValueError("Invalid measured output")
        if (index + 1) % 50 == 0:
            print(f"Measured {index + 1}/{args.iterations} calls per mode", flush=True)
    peak_rss = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss
    return {
        "model": args.worker_model, "status": "complete", "syntheticOnly": True,
        "settings": settings(args), "syntheticInputSha256": hashlib.sha256(images.tobytes()).hexdigest(),
        "weightSha256": weight_hash, "weightsMb": path.stat().st_size / 1_000_000,
        "loadSeconds": load_seconds, "onnxruntime": ort.__version__,
        "loadAverageBefore": load_before, "loadAverageAfter": os.getloadavg(),
        "peakProcessRamMb": peak_rss * (1 if sys.platform == "darwin" else 1024) / 1_000_000,
        "timings": {mode: latency_stats(values) for mode, values in samples.items()},
        "samplesMs": samples,
    }


def run_child(args, name):
    command = [sys.executable, str(Path(__file__).resolve()), "--worker-model", name,
               "--model-root", str(args.model_root), "--output", str(args.output / name)]
    if args.video:
        command.extend(["--video", str(args.video)])
    else:
        command.extend(["--iterations", str(args.iterations)])
    for option in ("warmup", "threads", "seed"):
        command.extend([f"--{option}", str(getattr(args, option))])
    log_path = args.output / f"{name}.log"
    started = time.perf_counter()
    try:
        with log_path.open("x", encoding="utf-8") as log:
            child = subprocess.Popen(command, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
            try:
                code = child.wait()
            except KeyboardInterrupt:
                child.terminate()
                try:
                    child.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait()
                raise
        if code:
            raise ValueError(f"Child exited with code {code}; see {log_path.name}")
        result = json.loads((args.output / name / "summary.json").read_text(encoding="utf-8"))
        if (result.get("status") != "complete" or result.get("model") != name
                or result.get("settings") != settings(args) or result.get("syntheticOnly") != (args.video is None)):
            raise ValueError("Invalid or incompatible child report")
        for mode in benchmark_video.MODES if args.video else MODES:
            values = result["samplesMs"][mode]
            expected = args.iterations
            if args.video:
                expected = result["eligibleFaces"] if mode in ("onnx", "faceEmbedding") else result["decodedFrames"]
            if len(values) != expected or any(not math.isfinite(value) or value <= 0 for value in values):
                raise ValueError("Invalid timing samples")
            if result["timings"][mode] != (latency_stats(values) if values else None):
                raise ValueError("Child statistics do not match timing samples")
        result.update(log=log_path.name, summary=f"{name}/summary.json", processSeconds=time.perf_counter() - started)
        return result
    except (OSError, ValueError, KeyError, TypeError) as error:
        return {"model": name, "status": "failed", "error": str(error), "log": log_path.name}


def markdown(report):
    lines = [f"# Synthetic recognizer speed ({report['status']})", "", NOTE, "",
             f"CPU: {table_cell(report['environment']['cpu'])}; threads: {report['settings']['intraOpThreads']}; "
             f"warmup: {report['settings']['warmup']}; measured calls/mode: {report['settings']['iterations']}.", "",
             "| Model | ONNX median, ms | ONNX p95, ms | Aligned embedding median, ms | Aligned p95, ms | Embeddings/s | Peak RAM, MB |",
             "|---|---:|---:|---:|---:|---:|---:|"]
    completed = sorted((row for row in report["results"] if row["status"] == "complete"),
                       key=lambda row: row["timings"]["alignedEmbedding"]["medianMs"])
    for row in completed:
        raw, aligned = (row["timings"][mode] for mode in MODES)
        values = [row["model"], f"{raw['medianMs']:.3f}", f"{raw['p95Ms']:.3f}",
                  f"{aligned['medianMs']:.3f}", f"{aligned['p95Ms']:.3f}",
                  f"{aligned['serialPerSecond']:.1f}", f"{row['peakProcessRamMb']:.1f}"]
        lines.append("| " + " | ".join(map(table_cell, values)) + " |")
    for row in report["results"]:
        if row["status"] != "complete":
            lines.extend(["", f"Failed: {table_cell(row['model'])}: {table_cell(row['error'])}"])
    if report["unavailableModels"]:
        lines.extend(["", "Not installed: " + ", ".join(report["unavailableModels"]) + "."])
    lines.extend(["", "Sorted by aligned-embedding median. Rate = 1000 / mean latency. "
                  "Peak RAM includes recognizer loading/imports, but no detector session. "
                  "This is one serial CPU run, not a controlled cross-device benchmark."])
    return "\n".join(lines) + "\n"


def main(argv=None):
    args = arguments(argv)
    os.umask(0o077)
    args.output.mkdir(parents=True, mode=0o700, exist_ok=False)
    if args.worker_model:
        result = benchmark_video.benchmark_video(args, latency_stats) if args.video else benchmark_model(args)
        result["settings"] = settings(args)
        write_json(args.output / "summary.json", result)
        return 0
    cpu = platform.processor() or platform.machine()
    if sys.platform == "darwin":
        cpu = subprocess.check_output(["/usr/sbin/sysctl", "-n", "machdep.cpu.brand_string"], text=True).strip()
    environment = {"cpu": cpu, "machine": platform.machine(), "system": platform.platform(),
                   "python": platform.python_version(), "logicalCpus": os.cpu_count(),
                   "loadAverageBefore": os.getloadavg()}
    results, interrupted = [], False
    print(f"{'Single-clip timing-only' if args.video else 'Synthetic-only'} benchmark; report directory: {args.output}", flush=True)
    try:
        for index, name in enumerate(args.model_set, 1):
            print(f"[{index}/{len(args.model_set)}] {name}; log: {args.output / (name + '.log')}", flush=True)
            result = run_child(args, name)
            results.append(result)
            write_json(args.output / "progress.json", {"results": results})
            if result["status"] == "complete":
                raw = result["timings"]["onnx"]
                print(f"  ONNX median: {raw['medianMs']:.3f} ms" if raw else "  No eligible faces for recognizer timing", flush=True)
            else:
                print(f"  FAILED: {result['error']}", flush=True)
    except KeyboardInterrupt:
        interrupted = True
    completed = [row for row in results if row["status"] == "complete"]
    signature_key = "samplingSignature" if args.video else "syntheticInputSha256"
    comparable = len({(row[signature_key], row["onnxruntime"]) for row in completed}) == 1
    all_complete = len(completed) == len(args.model_set) and comparable
    status = "interrupted" if interrupted else "complete" if all_complete else "incomplete"
    environment["loadAverageAfter"] = os.getloadavg()
    report = {"status": status, "syntheticOnly": args.video is None,
              "note": benchmark_video.NOTE if args.video else NOTE, "settings": settings(args),
              "videoFile": args.video.name if args.video else None,
              "environment": environment, "requestedModels": args.model_set,
              "unavailableModels": args.unavailable, "comparableInputsAndRuntime": comparable, "results": results}
    write_json(args.output / "benchmark.json", report)
    content = benchmark_video.markdown(report, table_cell) if args.video else markdown(report)
    with (args.output / "benchmark.md").open("x", encoding="utf-8") as stream:
        stream.write(content)
    print(content, flush=True)
    return 130 if interrupted else 0 if status == "complete" else 1


if __name__ == "__main__":
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        sys.exit(130)
    except (OSError, ValueError, ImportError, RuntimeError, AssertionError) as error:
        print(f"Error: {error}", file=sys.stderr)
        sys.exit(1)
