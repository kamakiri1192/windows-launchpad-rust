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
    r"dimension_scale=([0-9.]+) target_hz=([0-9.]+) "
    r"pixel_reduction=([0-9.]+)%"
)
TIME_RE = re.compile(r"^(real|user|sys) ([0-9]+(?:\.[0-9]+)?)$", re.MULTILINE)


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
    warmup_counts_path = directory / "runtime-warmup-counts.json"
    warmup_counts = {}
    if warmup_counts_path.is_file():
        data = json.loads(warmup_counts_path.read_text(encoding="utf-8"))
        if isinstance(data, dict):
            warmup_counts = data

    for path in sorted(directory.glob("*.log")):
        skipped: dict[str, int] = defaultdict(int)
        path_warmup = warmup_counts.get(path.name, {})
        if not isinstance(path_warmup, dict):
            path_warmup = {}
        for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
            group = None
            if "macOS capture stats:" in line:
                group = "macos_capture"
            elif "liquid glass stats:" in line:
                group = "liquid_glass"
            if group is None:
                continue
            warmup_limit = path_warmup.get(group, 0)
            if isinstance(warmup_limit, int) and skipped[group] < warmup_limit:
                skipped[group] += 1
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
                "target_hz": float(raw[9]),
                "pixel_reduction_percent": float(raw[10]),
                "observations": 0,
            }
            key = integer_values + (round(record["scale"] * 1000),)
            if key not in records:
                records[key] = record
            records[key]["observations"] += 1
    return list(records.values())


def load_qa_telemetry(directory: Path) -> dict[str, float | int] | None:
    runs = []
    for path in sorted(directory.glob("qa-sequences/*/manifest.json")):
        manifest = json.loads(path.read_text(encoding="utf-8"))
        frames = manifest.get("frames", [])
        if not isinstance(frames, list) or not frames:
            continue
        frame_dt = [
            value
            for frame in frames
            if isinstance(frame, dict)
            and (value := finite_number(frame.get("frame_dt_ms"))) is not None
        ]
        active = [
            frame
            for frame in frames
            if isinstance(frame, dict)
            and frame.get("folder_scroll_phase") not in (None, "Idle")
        ]
        active_dt = [
            value
            for frame in active
            if (value := finite_number(frame.get("frame_dt_ms"))) is not None
        ]
        relayouts = sum(
            int(frame.get("relayout_delta", 0))
            for frame in frames
            if isinstance(frame, dict)
        )
        active_relayouts = sum(int(frame.get("relayout_delta", 0)) for frame in active)
        runs.append(
            {
                "frames": len(frames),
                "active_frames": len(active),
                "frame_p95_ms": percentile(frame_dt, 0.95),
                "frame_max_ms": max(frame_dt, default=0.0),
                "active_frame_p95_ms": percentile(active_dt, 0.95),
                "relayouts": relayouts,
                "active_relayouts_per_frame": active_relayouts / max(1, len(active)),
            }
        )
    if not runs:
        return None
    return {
        "runs": len(runs),
        "min_frames": min(run["frames"] for run in runs),
        "min_active_frames": min(run["active_frames"] for run in runs),
        "median_frame_p95_ms": statistics.median(run["frame_p95_ms"] for run in runs),
        "max_frame_ms": max(run["frame_max_ms"] for run in runs),
        "median_active_frame_p95_ms": statistics.median(
            run["active_frame_p95_ms"] for run in runs
        ),
        "median_relayouts": statistics.median(run["relayouts"] for run in runs),
        "median_active_relayouts_per_frame": statistics.median(
            run["active_relayouts_per_frame"] for run in runs
        ),
    }


def load_qa_process_times(directory: Path) -> dict[str, float | int] | None:
    runs = []
    for path in sorted(directory.glob("qa-*.log")):
        values = {
            key: float(value)
            for key, value in TIME_RE.findall(
                path.read_text(encoding="utf-8", errors="replace")
            )
        }
        if all(key in values for key in ("real", "user", "sys")):
            runs.append(values)
    if not runs:
        return None
    return {
        "runs": len(runs),
        "median_real_seconds": statistics.median(run["real"] for run in runs),
        "median_user_seconds": statistics.median(run["user"] for run in runs),
        "median_system_seconds": statistics.median(run["sys"] for run in runs),
        "median_cpu_seconds": statistics.median(
            run["user"] + run["sys"] for run in runs
        ),
    }


def markdown(summary: dict[str, object]) -> str:
    lines = ["# macOS performance summary", ""]
    qa = summary.get("qa")
    if qa:
        lines += [
            "## QA frame telemetry",
            "",
            f"- Runs / minimum frames: {qa['runs']} / {qa['min_frames']}",
            f"- Active folder-scroll frames (minimum): {qa['min_active_frames']}",
            f"- Frame dt median p95 / max: {qa['median_frame_p95_ms']:.2f} / {qa['max_frame_ms']:.2f} ms",
            f"- Active folder-scroll frame dt median p95: {qa['median_active_frame_p95_ms']:.2f} ms",
            f"- Relayouts median / active relayouts per frame: {qa['median_relayouts']:.1f} / {qa['median_active_relayouts_per_frame']:.2f}",
            "",
        ]
    qa_process = summary.get("qa_process")
    if qa_process:
        lines += [
            "## QA process time",
            "",
            f"- Runs: {qa_process['runs']}",
            f"- Median real / user / system: {qa_process['median_real_seconds']:.2f} / {qa_process['median_user_seconds']:.2f} / {qa_process['median_system_seconds']:.2f} s",
            f"- Median CPU time (user + system): {qa_process['median_cpu_seconds']:.2f} s",
            "",
        ]
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
            "| Window | ROI | Output | dimension scale | target Hz | pixel reduction | observations |",
            "|---|---|---|---:|---:|---:|---:|",
        ]
        for geometry in geometries:
            lines.append(
                f"| {geometry['window_width']}x{geometry['window_height']} "
                f"| {geometry['roi_x']},{geometry['roi_y']} "
                f"{geometry['roi_width']}x{geometry['roi_height']} "
                f"| {geometry['output_width']}x{geometry['output_height']} "
                f"| {geometry['scale']:.2f} "
                f"| {geometry['target_hz']:.1f} "
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
        "qa": load_qa_telemetry(directory),
        "qa_process": load_qa_process_times(directory),
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
