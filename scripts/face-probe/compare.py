#!/usr/bin/env python3
"""Run selected local recognizers sequentially and report raw scores, time and RAM.

This is an experiment runner, not an accuracy benchmark or an access controller.
Models must be prepared separately; this command makes no model downloads.
"""

import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import time

from assets import BASE_MODELS, MODEL_FILES, ROOT, verify_models
import probe


def child_arguments(args, name):
    result = [str(args.video), "--references", str(args.references),
              "--models", str(args.model_root / "buffalo_l"),
              "--recognizer", str(args.model_root / MODEL_FILES[name]),
              "--output", str(args.output / name)]
    if args.all_frames:
        result.append("--all-frames")
    else:
        result.extend(["--sample-fps", str(args.sample_fps)])
    for key in ("det_size", "reference_det_size", "threads",
                "min_face", "min_eyes", "start", "end"):
        value = getattr(args, key)
        if value is not None:
            result.extend(["--" + key.replace("_", "-"), str(value)])
    return result


def arguments():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("video", type=Path)
    parser.add_argument("--references", type=Path, required=True)
    parser.add_argument("--model-root", type=Path, default=ROOT)
    parser.add_argument("--model-set", nargs="+", choices=list(MODEL_FILES), default=list(BASE_MODELS))
    parser.add_argument("--output", type=Path, help="New comparison directory")
    sampling = parser.add_mutually_exclusive_group()
    sampling.add_argument("--sample-fps", type=float, default=3)
    sampling.add_argument("--all-frames", action="store_true", help="Process every decoded frame")
    parser.add_argument("--det-size", type=int, default=640)
    parser.add_argument("--reference-det-size", type=int, default=640)
    parser.add_argument("--threads", type=int, default=3)
    parser.add_argument("--min-face", type=float, default=60)
    parser.add_argument("--min-eyes", type=float, default=20)
    parser.add_argument("--start", type=float, default=0)
    parser.add_argument("--end", type=float)
    args = parser.parse_args()
    if len(args.model_set) != len(set(args.model_set)):
        parser.error("--model-set must not contain duplicates")
    for name in ("video", "references", "model_root"):
        setattr(args, name, getattr(args, name).expanduser().resolve())
    timestamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S.%fZ")
    args.output = (args.output or probe.HERE / "reports" / f"compare-{timestamp}").expanduser().absolute()
    if args.output.exists() or args.output.is_symlink():
        parser.error("Output directory already exists; previous reports are never overwritten")
    # Reuse the single-model CLI's validation, before starting any subprocess.
    for name in args.model_set:
        probe.arguments(child_arguments(args, name))
    verify_models(args.model_root, args.model_set)
    return args


def sampling_signature(directory, summary):
    """Check that all runs used the same reference detections and video samples."""
    digest = hashlib.sha256()
    digest.update(json.dumps(summary["references"], sort_keys=True).encode())
    with (directory / "frames.jsonl").open(encoding="utf-8") as stream:
        for line in stream:
            frame = json.loads(line)
            payload = {key: frame[key] for key in ("frameIndex", "timeSec")}
            payload["faces"] = [
                {key: value for key, value in face.items() if key != "comparisons"}
                for face in frame["faces"]
            ]
            digest.update(json.dumps(payload, sort_keys=True).encode())
            digest.update(b"\n")
    return digest.hexdigest()


def run_one(args, name):
    log_path = args.output / f"{name}.log"
    command = [sys.executable, str(probe.HERE / "probe.py"), *child_arguments(args, name)]
    started = time.perf_counter()
    with log_path.open("x", encoding="utf-8") as log:
        process = subprocess.Popen(command, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
        try:
            returncode = process.wait()
        except KeyboardInterrupt:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
            raise
    result = {"model": name, "exitCode": returncode, "log": log_path.name,
              "processSeconds": round(time.perf_counter() - started, 2), "status": "failed"}
    if returncode != 0:
        result["error"] = "Model run failed; see the per-model log"
        return result
    directory = args.output / name
    try:
        summary = json.loads((directory / "summary.json").read_text(encoding="utf-8"))
        if summary.get("status") != "complete" or summary.get("mode") != "comparison":
            raise ValueError("Missing completed comparison summary")
        signature = sampling_signature(directory, summary)
        result.update({
            "status": "complete", "samplingSignature": signature,
            "sampledFrames": summary["sampledFrames"], "eligibleFaces": summary["eligibleFaces"],
            "weightsMb": round(summary["models"]["recognizer"]["bytes"] / 1_000_000, 1),
            "inferenceMedianMs": summary["inferenceMedianMs"],
            "peakProcessRamMb": summary["peakProcessRamMb"],
            "wallSeconds": summary["wallSeconds"],
            "bestScoresByLabel": summary["bestScoresByLabel"],
            "summary": f"{name}/summary.json",
        })
    except (OSError, ValueError, KeyError, TypeError) as error:
        result["error"] = f"Invalid or incomplete output: {error}"
    return result


def table_cell(value):
    # Labels are user-supplied folder names; keep them literal in the Markdown table.
    import html

    text = " ".join(str(value).split())
    text = "".join(character for character in text if character.isprintable())
    return html.escape(text).replace("|", "&#124;").replace("`", "&#96;")


def table(results):
    lines = ["| Model | Label | Max cosine | Best time, s | Frame median, ms | Total, s | Peak RAM, MB |",
             "|---|---|---:|---:|---:|---:|---:|"]
    for result in results:
        if result["status"] != "complete":
            cells = [result["model"], result["status"], "—", "—", "—", "—", "—"]
            lines.append("| " + " | ".join(table_cell(cell) for cell in cells) + " |")
            continue
        for label, best in result["bestScoresByLabel"].items() or [("no eligible faces", {})]:
            cells = [result["model"], label, best.get("maxCosine", "—"), best.get("timeSec", "—"),
                     result["inferenceMedianMs"], result["processSeconds"], result["peakProcessRamMb"]]
            lines.append("| " + " | ".join(table_cell(cell) for cell in cells) + " |")
    return "\n".join(lines)


def save_results(args, results, interrupted):
    completed = [result for result in results if result["status"] == "complete"]
    all_complete = len(completed) == len(args.model_set)
    same_samples = all_complete and len({result["samplingSignature"] for result in completed}) == 1
    status = "interrupted" if interrupted else "complete" if all_complete and same_samples else "incomplete"
    report = {
        "status": status, "sameSamplingAcrossModels": same_samples,
        "requestedModels": args.model_set, "results": results,
        "warning": "Raw cosine scores are not accuracy percentages and cannot rank different models by quality.",
    }
    probe.write_json(args.output / "comparison.json", report)
    content = (f"# Local comparison ({status})\n\n" + report["warning"] + "\n\n" + table(results)
               + f"\n\nSame sampling across all requested models: {same_samples}.\n"
               + "\nTotal includes child startup, loading, reference processing and video analysis. "
               + "Peak RAM is per child process, including model loading.\n")
    with (args.output / "comparison.md").open("x", encoding="utf-8") as stream:
        stream.write(content)
    print(content, flush=True)
    print(f"Report directory: {args.output}", flush=True)
    return 130 if interrupted else 0 if status == "complete" else 1


def main():
    args = arguments()
    os.umask(0o077)
    args.output.mkdir(parents=True, mode=0o700, exist_ok=False)
    results, interrupted = [], False
    try:
        for name in args.model_set:
            print(f"Running {name}; log: {args.output / (name + '.log')}", flush=True)
            results.append(run_one(args, name))
    except KeyboardInterrupt:
        interrupted = True
    return save_results(args, results, interrupted)


if __name__ == "__main__":
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        sys.exit(130)
    except (OSError, ValueError) as error:
        print(f"Error: {error}", file=sys.stderr)
        sys.exit(1)
