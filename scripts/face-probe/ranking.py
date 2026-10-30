"""Clip-level separation metrics and reports; no image or model processing."""

import statistics

from compare import table_cell


WARNING = (
    "Exploratory ranking on filename labels, not identity confirmation or production accuracy. "
    "Cosines are not percentages. ROC-AUC compares positive/negative clip ordering within each model. "
    "Do not derive a door-opening threshold from this small evaluation set."
)


def separation(positive, negative):
    """Exact empirical ROC-AUC: P(positive > negative) + 0.5 P(tie)."""
    wins = sum(p > n for p in positive for n in negative)
    ties = sum(p == n for p in positive for n in negative)
    pairs = len(positive) * len(negative)
    return {
        "auc": (wins + ties / 2) / pairs if pairs else None,
        "orderedPairs": wins, "tiedPairs": ties, "pairCount": pairs,
        "positiveMin": min(positive) if positive else None,
        "positiveMean": statistics.fmean(positive) if positive else None,
        "negativeMean": statistics.fmean(negative) if negative else None,
        "negativeMax": max(negative) if negative else None,
        "gap": min(positive) - max(negative) if pairs else None,
    }


def build_report(manifest, results, interrupted=False):
    samples, models = manifest["samples"], manifest["requestedModels"]
    lookup = {(result["sample"], result["model"]): result for result in results}
    sampling = {}
    for sample in samples:
        signatures = {
            result["samplingSignature"] for name in models
            if (result := lookup.get((sample["id"], name))) and result["status"] == "complete"
        }
        sampling[sample["id"]] = len(signatures) == 1 if signatures else None
    same_sampling = all(value is True for value in sampling.values())
    rows = []
    for name in models:
        runs = [lookup[(sample["id"], name)] for sample in samples if (sample["id"], name) in lookup]
        complete = [run for run in runs if run["status"] == "complete"]
        groups = {
            group: [run["score"] for run in complete if run["group"] == group and run["score"] is not None]
            for group in ("positive", "negative")
        }
        fully_scored = sum(map(len, groups.values())) == len(samples)
        metrics = separation(groups["positive"], groups["negative"])
        if not fully_scored or not same_sampling:
            # Never make missing/hard clips disappear from a ranked model's denominator.
            metrics.update(auc=None, orderedPairs=None, tiedPairs=None, pairCount=None)
        rows.append({
            "model": name, "rank": None, **metrics,
            "rankingStatus": ("incomplete_samples" if not fully_scored else
                              "sampling_mismatch" if not same_sampling else "ranked"),
            "positiveScored": len(groups["positive"]), "negativeScored": len(groups["negative"]),
            "completeRuns": len(complete), "failedRuns": len(runs) - len(complete),
            "pendingRuns": len(samples) - len(runs),
            "unscoredClips": [run["video"] for run in complete if run["score"] is None],
            "processSeconds": round(sum(run.get("processSeconds", 0) for run in runs), 2),
            "peakProcessRamMb": max((run["peakProcessRamMb"] for run in complete), default=None),
        })
    # Shared ranks for equal AUC; raw cosine/gap is deliberately not a tiebreaker.
    rows.sort(key=lambda row: (row["auc"] is None, -(row["auc"] or 0)))
    last_auc, last_rank = None, None
    for position, row in enumerate(rows, 1):
        if row["auc"] is not None:
            if row["auc"] != last_auc:
                last_rank = position
            row["rank"] = last_rank
            last_auc = row["auc"]
    complete = all(row["rank"] is not None for row in rows)
    status = "interrupted" if interrupted else "complete" if complete else "incomplete"
    return {
        **manifest, "status": status, "warning": WARNING,
        "sameSamplingAcrossCompletedRuns": same_sampling,
        "sameSamplingBySample": sampling, "ranking": rows, "results": results,
    }


def markdown(report):
    def number(value, places=6):
        return "—" if value is None else f"{value:.{places}f}"

    def cells(values):
        return "| " + " | ".join(table_cell(value) for value in values) + " |"

    counts = {group: sum(sample["group"] == group for sample in report["samples"])
              for group in ("positive", "negative")}
    lines = [f"# Clip model ranking ({report['status']})", "", WARNING, "",
             f"Positive prefix: {table_cell(report['positivePrefix'])}; "
             f"negative prefix: {table_cell(report['negativePrefix'])}. "
             f"Reference label: {table_cell(report['referenceLabel'])}.", "",
             "| Rank | Model | ROC-AUC | Scored + / − | Positive min | Negative max | Gap | Total, s | Peak RAM, MB |",
             "|---:|---|---:|---|---:|---:|---:|---:|---:|"]
    for row in report["ranking"]:
        coverage = (f"{row['positiveScored']}/{counts['positive']} / "
                    f"{row['negativeScored']}/{counts['negative']}")
        lines.append(cells([row["rank"] or "—", row["model"], number(row["auc"], 4), coverage,
                            number(row["positiveMin"]), number(row["negativeMax"]), number(row["gap"]),
                            number(row["processSeconds"], 2), number(row["peakProcessRamMb"], 1)]))
    lines.extend([
        "", "Rank uses ROC-AUC only; equal AUC shares a rank. 1.0 means all positive clip scores "
        "exceed all negative clip scores here, not 100% accuracy on future videos.",
        "Gap = lowest positive minus highest negative cosine. A positive gap separates these clips; "
        "gap magnitudes across different models are not a common quality scale.",
        "Missing scores are not zero. Models with failed/unscored/pending clips get no rank. "
        "Min/max diagnostics for those models describe only their scored clips.",
        "Total includes all per-clip child startups, loading and analysis. Peak RAM is the maximum "
        "over those children, not their sum.", "", "## Per-clip maximum cosine", "",
        cells(["Clip", "Group", *report["requestedModels"]]),
        cells(["---", "---", *["---:" for _ in report["requestedModels"]]]),
    ])
    lookup = {(result["sample"], result["model"]): result for result in report["results"]}
    for sample in report["samples"]:
        values = [sample["video"], sample["group"]]
        for name in report["requestedModels"]:
            run = lookup.get((sample["id"], name))
            if not run:
                value = "pending"
            elif run["status"] != "complete":
                value = "failed"
            elif run["score"] is None:
                value = "no eligible faces"
            else:
                value = number(run["score"])
            values.append(value)
        lines.append(cells(values))
    lines.extend([
        "", "Each clip contributes one maximum over eligible faces, selected frames and reference photos. "
        "There is no person tracking: in a multi-person clip the maximum may come from any face. "
        "Longer clips and more references offer more chances for a high maximum.",
        "Use independent reference photos and separate held-out videos for validation. "
        "Adjacent frames, repeated visits and these pair comparisons are not independent people.",
        "", f"Same sampling/detections across completed runs: {report['sameSamplingAcrossCompletedRuns']}.",
    ])
    skipped = report["unavailableModels"]
    if skipped:
        lines.extend(["", "Not installed (not ranked): " + ", ".join(map(table_cell, skipped)) + "."])
    if report["ignoredVideos"]:
        lines.extend(["", "Ignored videos (no matching prefix): "
                      + ", ".join(map(table_cell, report["ignoredVideos"])) + "."])
    problems = [row for row in report["ranking"] if row["rank"] is None]
    if problems:
        lines.extend(["", "## Incomplete or non-comparable models", ""])
        for row in problems:
            lines.append(f"- {table_cell(row['model'])}: {row['rankingStatus']}; "
                         f"failed={row['failedRuns']}, unscored={len(row['unscoredClips'])}, "
                         f"pending={row['pendingRuns']}. See per-clip logs and ranking.json.")
    return "\n".join(lines) + "\n"
