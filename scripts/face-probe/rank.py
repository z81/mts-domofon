#!/usr/bin/env python3
"""Rank installed models by separation of filename-labelled positive/negative clips.

Default: ya* = positive, clip* = negative, every frame, all installed models.
No downloads, camera, door control, identity decisions or fitted access threshold.
"""

import argparse
from datetime import datetime, timezone
import math
import os
from pathlib import Path
import sys

from assets import MODEL_FILES, ROOT, verify_models
import compare
import probe
import ranking
from reference_inputs import reference_files


SETTINGS = ("all_frames", "sample_fps", "det_size", "reference_det_size", "threads",
            "min_face", "min_eyes", "start", "end")


def discover_samples(directory, positive_prefix, negative_prefix):
    if not directory.is_dir():
        raise ValueError(f"Video directory does not exist: {directory}")
    positive_prefix, negative_prefix = positive_prefix.casefold(), negative_prefix.casefold()
    if (not positive_prefix or not negative_prefix or positive_prefix.startswith(negative_prefix)
            or negative_prefix.startswith(positive_prefix)):
        raise ValueError("Positive and negative prefixes must be nonempty and must not overlap")
    samples, ignored = [], []
    for path in sorted(directory.iterdir(), key=lambda path: (path.name.casefold(), path.name)):
        if not path.is_file() or path.name.startswith(".") or path.suffix.lower() not in probe.VIDEO_SUFFIXES:
            continue
        if path.name.casefold().startswith(positive_prefix):
            group = "positive"
        elif path.name.casefold().startswith(negative_prefix):
            group = "negative"
        else:
            ignored.append(path.name)
            continue
        samples.append({"video": path.name, "group": group})
    if {sample["group"] for sample in samples} != {"positive", "negative"}:
        raise ValueError("Need at least one positive and one negative video with the selected prefixes")
    samples.sort(key=lambda sample: (sample["group"] != "positive", sample["video"].casefold()))
    return [{"id": f"clip-{index:03d}", **sample} for index, sample in enumerate(samples, 1)], ignored


def job_arguments(args, sample):
    return argparse.Namespace(**{**vars(args), "video": args.directory / sample["video"],
                                 "output": args.output / sample["id"]})


def arguments(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path, help="Flat directory containing labelled local videos")
    parser.add_argument("--references", type=Path, required=True, help="One photo or an album for ONE reference label")
    parser.add_argument("--model-root", type=Path, default=ROOT)
    parser.add_argument("--model-set", nargs="+", choices=list(MODEL_FILES), help="Default: all installed models")
    parser.add_argument("--positive-prefix", default="ya")
    parser.add_argument("--negative-prefix", default="clip")
    parser.add_argument("--output", type=Path, help="New report directory")
    sampling = parser.add_mutually_exclusive_group()
    sampling.add_argument("--sample-fps", type=float, help="Subsample instead of processing every frame")
    sampling.add_argument("--all-frames", action="store_true", help="Process every decoded frame (default)")
    parser.add_argument("--det-size", type=int, default=640)
    parser.add_argument("--reference-det-size", type=int, default=640)
    parser.add_argument("--threads", type=int, default=3)
    parser.add_argument("--min-face", type=float, default=60)
    parser.add_argument("--min-eyes", type=float, default=20)
    parser.add_argument("--start", type=float, default=0)
    parser.add_argument("--end", type=float)
    parser.add_argument("--dry-run", action="store_true", help="Validate file inventory and weights without decoding or inference")
    args = parser.parse_args(argv)
    args.all_frames = args.sample_fps is None
    args.sample_fps = 3 if args.all_frames else args.sample_fps
    for name in ("directory", "references", "model_root"):
        setattr(args, name, getattr(args, name).expanduser().resolve())
    timestamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S.%fZ")
    args.output = (args.output or probe.HERE / "reports" / f"rank-{timestamp}").expanduser().absolute()
    if args.output.exists() or args.output.is_symlink():
        parser.error("Output directory already exists; previous reports are never overwritten")
    args.samples, args.ignored = discover_samples(args.directory, args.positive_prefix, args.negative_prefix)
    args.reference_inventory = reference_files(args.references)
    labels = {label for label, _, _ in args.reference_inventory}
    if len(labels) != 1:
        parser.error("Ranking needs ONE reference label; pass that person's photo folder with --references")
    args.reference_label = next(iter(labels))
    installed = [name for name, path in MODEL_FILES.items()
                 if (args.model_root / path).exists() or (args.model_root / path).is_symlink()]
    args.unavailable = [name for name in MODEL_FILES if name not in installed]
    args.model_set = installed if args.model_set is None else args.model_set
    if not args.model_set:
        parser.error("No installed models found; run prepare_models.py first")
    if len(set(args.model_set)) != len(args.model_set):
        parser.error("--model-set must not contain duplicates")
    verify_models(args.model_root, args.model_set)
    for sample in args.samples:
        job = job_arguments(args, sample)
        for name in args.model_set:
            probe.arguments(compare.child_arguments(job, name))
    return args


def manifest(args):
    return {
        "requestedModels": args.model_set, "unavailableModels": args.unavailable,
        "positivePrefix": args.positive_prefix, "negativePrefix": args.negative_prefix,
        "samples": args.samples, "ignoredVideos": args.ignored,
        "referenceLabel": args.reference_label,
        "referenceFiles": [relative for _, _, relative in args.reference_inventory],
        "settings": {key: getattr(args, key) for key in SETTINGS},
        "scoreAggregation": "Maximum cosine over eligible faces, selected frames and reference photos per clip",
    }


def run_job(args, sample, name):
    job = job_arguments(args, sample)
    job.output.mkdir(mode=0o700, exist_ok=True)
    try:
        result = compare.run_one(job, name)
    except OSError as error:
        result = {"model": name, "status": "failed", "error": str(error), "log": f"{name}.log"}
    result.update(sample=sample["id"], video=sample["video"], group=sample["group"], score=None)
    for key in ("log", "summary"):
        if key in result:
            result[key] = f"{sample['id']}/{result[key]}"
    if result["status"] == "complete":
        try:
            best = result["bestScoresByLabel"].get(args.reference_label)
            if best is None:
                if result["eligibleFaces"] != 0:
                    raise ValueError("Eligible faces present but reference label score is missing")
                result["unscoredReason"] = "no_eligible_faces"
            else:
                score = best["maxCosine"]
                if (isinstance(score, bool) or not isinstance(score, (float, int))
                        or not math.isfinite(score) or not -1 <= score <= 1):
                    raise ValueError("Invalid cosine in child report")
                result.update(score=score, bestTimeSec=best["timeSec"])
        except (ValueError, KeyError, TypeError, AttributeError) as error:
            result.update(status="failed", score=None, error=f"Invalid score report: {error}")
    return result


def main(argv=None):
    args = arguments(argv)
    plan = manifest(args)
    positive = sum(sample["group"] == "positive" for sample in args.samples)
    jobs = len(args.samples) * len(args.model_set)
    print(f"Dataset: {positive} positive, {len(args.samples) - positive} negative clips; "
          f"{len(args.model_set)} models; {jobs} jobs.", flush=True)
    print("Models: " + ", ".join(args.model_set), flush=True)
    print("Sampling: " + ("every decoded frame" if args.all_frames else f"{args.sample_fps:g} FPS"), flush=True)
    if args.unavailable:
        print("Not installed: " + ", ".join(args.unavailable), flush=True)
    if args.ignored:
        print("Ignored (no matching prefix): " + ", ".join(args.ignored), flush=True)
    if args.dry_run:
        for sample in args.samples:
            print(f"  {sample['group']}: {sample['video']}")
        print("Dry run complete. No images/videos decoded, no inference and no report directory created.")
        return 0
    os.umask(0o077)
    args.output.mkdir(parents=True, mode=0o700, exist_ok=False)
    probe.write_json(args.output / "manifest.json", plan)
    print(f"Report directory: {args.output}", flush=True)
    results, interrupted = [], False
    try:
        for sample in args.samples:
            for name in args.model_set:
                log = args.output / sample["id"] / f"{name}.log"
                print(f"[{len(results) + 1}/{jobs}] {sample['video']} ({sample['group']}) / {name}; log: {log}", flush=True)
                result = run_job(args, sample, name)
                results.append(result)
                probe.write_json(args.output / "progress.json", {"completedJobs": len(results), "totalJobs": jobs,
                                                               "results": results})
                if result["status"] != "complete":
                    print(f"  FAILED: {result['error']}", flush=True)
                elif result["score"] is None:
                    print("  No eligible faces: no score (not zero).", flush=True)
    except KeyboardInterrupt:
        interrupted = True
    report = ranking.build_report(plan, results, interrupted)
    probe.write_json(args.output / "ranking.json", report)
    content = ranking.markdown(report)
    with (args.output / "ranking.md").open("x", encoding="utf-8") as stream:
        stream.write(content)
    print(content, flush=True)
    print(f"Report directory: {args.output}", flush=True)
    return 130 if interrupted else 0 if report["status"] == "complete" else 1


if __name__ == "__main__":
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        sys.exit(130)
    except (OSError, ValueError) as error:
        print(f"Error: {error}", file=sys.stderr)
        sys.exit(1)
