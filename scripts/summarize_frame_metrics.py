#!/usr/bin/env python3
"""Summarize opt-in production frame timings, independently of GPU timestamps."""
import argparse
import csv
import json
import statistics
from pathlib import Path


def summarize(path, warmup):
    with path.open() as stream:
        rows = [{k: float(v) for k, v in row.items()} for row in csv.DictReader(stream)]
    rows = [row for row in rows if row["elapsed_s"] >= warmup]
    if not rows:
        raise ValueError(f"no post-warmup frames: {path}")
    result = {"path": str(path), "warmup_s": warmup, "frames": len(rows)}
    for key in rows[0]:
        if key == "elapsed_s":
            continue
        values = sorted(row[key] for row in rows)
        result[key] = {
            "mean": statistics.mean(values),
            "p50": statistics.median(values),
            "p95": values[round((len(values) - 1) * .95)],
            "p99": values[round((len(values) - 1) * .99)],
            "stddev": statistics.pstdev(values),
            "max": values[-1],
        }
    result["present_call_fps"] = 1000 / result["interval_ms"]["mean"]
    return result


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("csv", type=Path, nargs="+")
    parser.add_argument("--warmup", type=float, default=10)
    args = parser.parse_args()
    print(json.dumps([summarize(path, args.warmup) for path in args.csv], indent=2))
