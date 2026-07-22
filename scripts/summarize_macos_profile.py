#!/usr/bin/env python3
"""Summarize repeatable macOS Launchpad performance runs."""

from __future__ import annotations

import csv
import json
import math
import re
import statistics
import sys
from collections import defaultdict
from pathlib import Path


METRIC_RE = re.compile(r"([a-z_]+)=(-?[0-9]+(?:\.[0-9]+)?)%?")
CAPTURE_GEOMETRY_RE = re.compile(
    r"macOS capture geometry: window=(\d+)x(\d+) "
    r"roi=(\d+),(\d+) (\d+)x(\d+) output=(\d+)x(\d+) "
    r"scale=([0-9.]+) pixel_reduction=([0-9.]+)%"
)


def percentile(values: list[float], fraction: float) -> float:
    if not values:
        return 0.0
    ordered = sorted(values)
    index = round((len(ordered) - 1) * fraction)
    return ordered[index]


def finite_number(value: object) -> float | None:
    if not isinstance(value, (int, float)):
        return None
    number = float(value)
    return number if math.isfinite(number) else None


def load_gpu_reports(directory: Path) -> dict[str, object]:
    reports = []
    for path in sorted(directory.glob("*.json")):
        if path.name.endswith(".trace.json") or path.name == "summary.json":
            continue
        data = json.loads(path.read_text(encoding="utf-8"))
        if isinstance(data, dict) and isinstance(data.get("scopes"), dict):
            reports.append((path.name, data))

    scopes: dict[str, list[dict[str, object]]] = defaultdict(list)
    for _, report in reports:
        for label, metrics in report["scopes"].items():
            if isinstance(metrics, dict):
                scopes[label].append(metrics)

    aggregate = {}
    for label, runs in scopes.items():
        p50s = [value for run in runs if (value := finite_number(run.get("p50_ms"))) is not None]
        p95s = [value for run in runs if (value := finite_number(run.get("p95_ms"))) is not None]
        maxima = [value for run in runs if (value := finite_number(run.get("max_ms"))) is not None]
        aggregate[label] = {
            "runs": len(runs),
            "min_samples": min(int(run.get("samples", 0)) for run in runs),
            "invalid_samples": sum(int(run.get("invalid_samples", 0)) for run in runs),
            "median_p50_ms": statistics.median(p50s) if p50s else 0.0,
            "median_p95_ms": statistics.median(p95s) if p95s else 0.0,
            "max_ms": max(maxima, default=0.0),
        }

    return {
        "reports": [name for name, _ in reports],
        "finished_frames": [int(report.get("finished_frames", 0)) for _, report in reports],
        "scopes": aggregate,
    }


def load_process_samples(directory: Path) -> dict[str, float | int] | None:
    path = directory / "process.csv"
    if not path.is_file():
        return None
    cpu = []
    rss_mb = []
    with path.open(newline="", encoding="utf-8") as handle:
        for row in csv.DictReader(handle):
            try:
                cpu.append(float(row["cpu_percent"]))
                rss_mb.append(float(row["rss_kb"]) / 1024.0)
            except (KeyError, ValueError):
                continue
    if not cpu:
        return None
    return {
        "samples": len(cpu),
        "cpu_average_percent": statistics.mean(cpu),
        "cpu_p95_percent": percentile(cpu, 0.95),
        "rss_average_mb": statistics.mean(rss_mb),
        "rss_p95_mb": percentile(rss_mb, 0.95),
        "rss_max_mb": max(rss_mb),
    }


def load_runtime_metrics(directory: Path) -> dict[str, dict[str, float | int]]:
    grouped: dict[str, dict[str, list[float]]] = {
        "macos_capture": defaultdict(list),
        "liquid_glass": defaultdict(list),
    }
    for path in sorted(directory.glob("*.log")):
        for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
            group = None
            if "macOS capture stats:" in line:
                group = "macos_capture"
            elif "liquid glass stats:" in line:
                group = "liquid_glass"
            if group is None:
                continue
            for key, raw in METRIC_RE.findall(line):
                grouped[group][key].append(float(raw))

    result = {}
    for group, metrics in grouped.items():
        if not metrics:
            continue
        result[group] = {
            key: {
                "samples": len(values),
                "average": statistics.mean(values),
                "p95": percentile(values, 0.95),
                "max": max(values),
            }
            for key, values in metrics.items()
        }
    return result


def load_capture_geometry(directory: Path) -> list[dict[str, float | int]]:
    records: dict[tuple[int, ...], dict[str, float | int]] = {}
    for path in sorted(directory.glob("*.log")):
        for match in CAPTURE_GEOMETRY_RE.finditer(
            path.read_text(encoding="utf-8", errors="replace")
        ):
            raw = match.groups()
            integer_values = tuple(int(value) for value in raw[:8])
            record = {
                "window_width": integer_values[0],
                "window_height": integer_values[1],
                "roi_x": integer_values[2],
                "roi_y": integer_values[3],
                "roi_width": integer_values[4],
                "roi_height": integer_values[5],
                "output_width": integer_values[6],
                "output_height": integer_values[7],
                "scale": float(raw[8]),
                "pixel_reduction_percent": float(raw[9]),
                "observations": 0,
            }
            key = integer_values + (round(record["scale"] * 1000),)
            if key not in records:
                records[key] = record
            records[key]["observations"] += 1
    return list(records.values())


def markdown(summary: dict[str, object]) -> str:
    lines = ["# macOS performance summary", ""]
    process = summary.get("process")
    if process:
        lines += [
            "## Process",
            "",
            f"- Samples: {process['samples']}",
            f"- CPU average / p95: {process['cpu_average_percent']:.2f}% / {process['cpu_p95_percent']:.2f}%",
            f"- RSS average / p95 / max: {process['rss_average_mb']:.1f} / {process['rss_p95_mb']:.1f} / {process['rss_max_mb']:.1f} MiB",
            "",
        ]

    runtime = summary.get("runtime", {})
    if runtime:
        lines += ["## Runtime logs", ""]
        for group, metrics in runtime.items():
            lines.append(f"### {group}")
            lines.append("")
            for key, values in sorted(metrics.items()):
                lines.append(
                    f"- {key}: avg {values['average']:.2f}, p95 {values['p95']:.2f}, max {values['max']:.2f} ({values['samples']} samples)"
                )
            lines.append("")

    geometries = summary.get("capture_geometry", [])
    if geometries:
        lines += [
            "## Capture geometry",
            "",
            "| Window | ROI | Output | scale | pixel reduction | observations |",
            "|---|---|---|---:|---:|---:|",
        ]
        for geometry in geometries:
            lines.append(
                f"| {geometry['window_width']}x{geometry['window_height']} "
                f"| {geometry['roi_x']},{geometry['roi_y']} "
                f"{geometry['roi_width']}x{geometry['roi_height']} "
                f"| {geometry['output_width']}x{geometry['output_height']} "
                f"| {geometry['scale']:.2f} "
                f"| {geometry['pixel_reduction_percent']:.1f}% "
                f"| {geometry['observations']} |"
            )
        lines.append("")

    gpu = summary["gpu"]
    scopes = gpu["scopes"]
    if scopes:
        lines += ["## GPU timestamp scopes", "", "| Scope | runs | min samples | median p50 ms | median p95 ms | max ms | invalid |", "|---|---:|---:|---:|---:|---:|---:|"]
        ranked = sorted(scopes.items(), key=lambda item: item[1]["median_p95_ms"], reverse=True)
        for label, values in ranked:
            lines.append(
                f"| {label} | {values['runs']} | {values['min_samples']} | {values['median_p50_ms']:.4f} | {values['median_p95_ms']:.4f} | {values['max_ms']:.4f} | {values['invalid_samples']} |"
            )
        lines.append("")
    return "\n".join(lines)


def main() -> int:
    if len(sys.argv) != 2:
        print(f"usage: {Path(sys.argv[0]).name} PROFILE_DIRECTORY", file=sys.stderr)
        return 2
    directory = Path(sys.argv[1])
    summary = {
        "gpu": load_gpu_reports(directory),
        "process": load_process_samples(directory),
        "runtime": load_runtime_metrics(directory),
        "capture_geometry": load_capture_geometry(directory),
    }
    (directory / "summary.json").write_text(
        json.dumps(summary, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )
    rendered = markdown(summary)
    (directory / "summary.md").write_text(rendered + "\n", encoding="utf-8")
    print(rendered)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
