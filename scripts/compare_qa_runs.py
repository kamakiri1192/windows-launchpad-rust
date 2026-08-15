#!/usr/bin/env python3
"""Compare two QA scenario runs frame-by-frame for pixel-level regression.

Usage:
    python3 scripts/compare_qa_runs.py <baseline_run_dir> <candidate_run_dir> \
        [--tolerance 2] [--max-frames N] [--report <path>] \
        [--mean-threshold 1.0] [--changed-pct-threshold 0.5]

Pairs frames by index (frame_000000.png, frame_000001.png, ...). Each run dir
is produced by the launcher's QA runner under target/qa-sequences/<name>-<ts>/
and contains manifest.json plus frame_%06d.png files.

Per paired frame the script computes:
    mean_abs_diff     mean per-channel absolute difference over all pixels
    max_abs_diff      largest per-channel absolute difference
    changed_pixels_pct  % of pixels where any channel differs by > tolerance
    mean_abs_diff_d4  mean per-channel abs diff on both frames downscaled 1/4
                      (LANCZOS), which ignores sub-pixel jitter while keeping
                      real layout changes visible

The run PASSES when the overall (across-frame) mean_abs_diff <= --mean-threshold
AND the overall mean changed_pixels_pct <= --changed-pct-threshold.

Exit code: 0 = pass, 1 = fail, 2 = usage/runtime error.

--report <path>: writes the full per-frame table. Format follows the file
extension: .json for JSON, anything else (default .md) for Markdown.

numpy is strongly preferred for speed; the script falls back to a pure-Python
loop with Pillow's ImageChops if numpy is unavailable.
"""

import argparse
import json
import os
import re
import sys

try:
    import numpy as np

    HAS_NUMPY = True
except ImportError:  # pragma: no cover
    np = None
    HAS_NUMPY = False

try:
    from PIL import Image, ImageChops
except ImportError:  # pragma: no cover
    print("error: Pillow is required (pip install --user pillow)", file=sys.stderr)
    sys.exit(2)

FRAME_RE = re.compile(r"^frame_(\d+)\.png$")


def load_manifest(run_dir):
    path = os.path.join(run_dir, "manifest.json")
    if not os.path.exists(path):
        return None
    try:
        with open(path, "r", encoding="utf-8") as fh:
            return json.load(fh)
    except (OSError, json.JSONDecodeError) as exc:
        print(f"warning: cannot parse {path}: {exc}", file=sys.stderr)
        return None


def err_exit(message):
    print(f"error: {message}", file=sys.stderr)
    raise SystemExit(2)


def list_frames(run_dir):
    frames = {}
    if not os.path.isdir(run_dir):
        err_exit(f"run directory does not exist: {run_dir}")
    for entry in os.listdir(run_dir):
        match = FRAME_RE.match(entry)
        if match:
            frames[int(match.group(1))] = os.path.join(run_dir, entry)
    if not frames:
        err_exit(f"no frame_*.png files found in {run_dir} (is this a QA run dir?)")
    return frames


def load_rgb(path, downscale=1):
    img = Image.open(path).convert("RGB")
    if downscale > 1:
        img = img.resize(
            (max(1, img.width // downscale), max(1, img.height // downscale)),
            Image.LANCZOS,
        )
    return np.asarray(img, dtype=np.uint8)


def compare_pair(path_a, path_b, tolerance):
    if not os.path.exists(path_a) or not os.path.exists(path_b):
        err_exit(
            f"frame files missing ({os.path.basename(path_a)} vs "
            f"{os.path.basename(path_b)})"
        )
    im_a = load_rgb(path_a)
    im_b = load_rgb(path_b)
    if im_a.shape != im_b.shape:
        err_exit(
            f"frame size mismatch for {os.path.basename(path_a)}: "
            f"{im_a.shape[1]}x{im_a.shape[0]} vs {im_b.shape[1]}x{im_b.shape[0]}"
        )
    diff = np.abs(im_a.astype(np.int16) - im_b.astype(np.int16))
    mean_abs = float(diff.mean())
    max_abs = int(diff.max())
    changed = float((diff.max(axis=2) > tolerance).mean() * 100.0)
    # 1/4-downscale perceptual diff (LANCZOS on the loaded RGB frames).
    im_a4 = load_rgb(path_a, downscale=4)
    im_b4 = load_rgb(path_b, downscale=4)
    diff4 = np.abs(im_a4.astype(np.int16) - im_b4.astype(np.int16))
    mean_abs_d4 = float(diff4.mean())
    return {
        "mean_abs_diff": mean_abs,
        "max_abs_diff": max_abs,
        "changed_pixels_pct": changed,
        "mean_abs_diff_d4": mean_abs_d4,
    }


def compare_pair_fallback(path_a, path_b, tolerance):
    """Pure-Python fallback using ImageChops.difference + histogram."""
    if not os.path.exists(path_a) or not os.path.exists(path_b):
        err_exit(
            f"frame files missing ({os.path.basename(path_a)} vs "
            f"{os.path.basename(path_b)})"
        )
    im_a = Image.open(path_a).convert("RGB")
    im_b = Image.open(path_b).convert("RGB")
    if im_a.size != im_b.size:
        err_exit(
            f"frame size mismatch for {os.path.basename(path_a)}: "
            f"{im_a.size} vs {im_b.size}"
        )
    total_pixels = im_a.width * im_a.height
    diff = ImageChops.difference(im_a, im_b)
    hist = diff.histogram()  # 256 bins per channel, RGB in order
    per_channel = [hist[c * 256 : (c + 1) * 256] for c in range(3)]
    weighted = [sum(level * count for level, count in enumerate(channel)) for channel in per_channel]
    mean_abs = float(sum(weighted) / (3 * total_pixels))
    max_abs = max(
        next((i for i in range(255, -1, -1) if per_channel[c][i] > 0), 0)
        for c in range(3)
    )
    # Pixels where ANY channel exceeds the tolerance.
    mask = diff.point(lambda v: 255 if v > tolerance else 0)
    r, g, b = mask.split()
    any_mask = ImageChops.lighter(ImageChops.lighter(r, g), b)
    changed = sum(1 for value in any_mask.getdata() if value > 0) / total_pixels * 100.0
    d4_a = im_a.resize(
        (max(1, im_a.width // 4), max(1, im_a.height // 4)), Image.LANCZOS
    )
    d4_b = im_b.resize(
        (max(1, im_b.width // 4), max(1, im_b.height // 4)), Image.LANCZOS
    )
    diff4 = ImageChops.difference(d4_a, d4_b)
    hist4 = diff4.histogram()
    per_channel4 = [hist4[c * 256 : (c + 1) * 256] for c in range(3)]
    weighted4 = [
        sum(level * count for level, count in enumerate(channel))
        for channel in per_channel4
    ]
    total4 = d4_a.width * d4_a.height
    mean_abs_d4 = float(sum(weighted4) / (3 * total4))
    return {
        "mean_abs_diff": mean_abs,
        "max_abs_diff": int(max_abs),
        "changed_pixels_pct": changed,
        "mean_abs_diff_d4": mean_abs_d4,
    }


def parse_args(argv):
    parser = argparse.ArgumentParser(
        description="Compare two QA scenario runs frame-by-frame."
    )
    parser.add_argument("baseline_run_dir", help="baseline QA run directory")
    parser.add_argument("candidate_run_dir", help="candidate QA run directory")
    parser.add_argument(
        "--tolerance", type=int, default=2,
        help="per-channel diff threshold for a pixel to count as changed (default 2)",
    )
    parser.add_argument(
        "--max-frames", type=int, default=None,
        help="only compare the first N frames (default: all paired frames)",
    )
    parser.add_argument(
        "--report", default=None,
        help="write the per-frame table to a file (.json = JSON, else Markdown)",
    )
    parser.add_argument(
        "--mean-threshold", type=float, default=1.0,
        help="PASS requires overall mean_abs_diff <= this (default 1.0)",
    )
    parser.add_argument(
        "--changed-pct-threshold", type=float, default=0.5,
        help="PASS requires overall changed_pixels_pct <= this (default 0.5)",
    )
    return parser.parse_args(argv)


def main(argv=None):
    args = parse_args(sys.argv[1:] if argv is None else argv)
    if args.tolerance < 0:
        print("error: --tolerance must be >= 0", file=sys.stderr)
        return 2

    manifest_a = load_manifest(args.baseline_run_dir)
    manifest_b = load_manifest(args.candidate_run_dir)

    if manifest_a and manifest_b:
        if manifest_a.get("scenario") != manifest_b.get("scenario"):
            print(
                f"error: scenario mismatch: baseline='{manifest_a.get('scenario')}' "
                f"candidate='{manifest_b.get('scenario')}'",
                file=sys.stderr,
            )
            return 2
        if manifest_a.get("viewport") != manifest_b.get("viewport"):
            print(
                f"warning: viewport differs: {manifest_a.get('viewport')} vs "
                f"{manifest_b.get('viewport')}",
                file=sys.stderr,
            )
        if not manifest_a.get("completed"):
            print("warning: baseline manifest completed=false", file=sys.stderr)
        if not manifest_b.get("completed"):
            print("warning: candidate manifest completed=false", file=sys.stderr)

    frames_a = list_frames(args.baseline_run_dir)
    frames_b = list_frames(args.candidate_run_dir)
    shared = sorted(set(frames_a) & set(frames_b))
    if not shared:
        print("error: the two runs share no frame indices", file=sys.stderr)
        return 2
    if len(frames_a) != len(frames_b):
        print(
            f"warning: frame counts differ (baseline {len(frames_a)} vs candidate "
            f"{len(frames_b)}); comparing the shared {len(shared)} frames",
            file=sys.stderr,
        )
    if args.max_frames is not None:
        if args.max_frames <= 0:
            print("error: --max-frames must be > 0", file=sys.stderr)
            return 2
        shared = [i for i in shared if i < args.max_frames]
        if not shared:
            print("error: --max-frames leaves no frames to compare", file=sys.stderr)
            return 2

    compare = compare_pair if HAS_NUMPY else compare_pair_fallback

    rows = []
    try:
        for index in shared:
            rows.append(
                (index, compare(frames_a[index], frames_b[index], args.tolerance))
            )
    except SystemExit as exc:
        # Already printed; propagate the error code.
        raise exc

    overall_mean = sum(r["mean_abs_diff"] for _, r in rows) / len(rows)
    overall_max = max(r["max_abs_diff"] for _, r in rows)
    overall_changed = sum(r["changed_pixels_pct"] for _, r in rows) / len(rows)
    overall_mean_d4 = sum(r["mean_abs_diff_d4"] for _, r in rows) / len(rows)
    worst = max(rows, key=lambda item: item[1]["mean_abs_diff"])

    passed = overall_mean <= args.mean_threshold and overall_changed <= args.changed_pct_threshold
    verdict = "PASS" if passed else "FAIL"

    print("=" * 78)
    print("QA frame comparison")
    print("=" * 78)
    print(f"baseline : {os.path.abspath(args.baseline_run_dir)}")
    print(f"candidate: {os.path.abspath(args.candidate_run_dir)}")
    if manifest_a and manifest_b:
        print(
            f"scenario : {manifest_a.get('scenario')}  "
            f"viewport {manifest_a.get('viewport')}  "
            f"fps {manifest_a.get('fps')}"
        )
    print(f"frames compared: {len(rows)}  (tolerance {args.tolerance}, "
          f"{'numpy' if HAS_NUMPY else 'pure-python'})")
    print("-" * 78)
    print(
        f"{'frame':>8} {'mean_abs':>10} {'mean_abs_d4':>12} {'max_abs':>8} "
        f"{'changed%':>10}"
    )
    for index, row in rows:
        print(
            f"{index:>8} {row['mean_abs_diff']:>10.4f} {row['mean_abs_diff_d4']:>12.4f} "
            f"{row['max_abs_diff']:>8} {row['changed_pixels_pct']:>10.4f}"
        )
    print("-" * 78)
    print(
        f"summary: mean_abs_diff={overall_mean:.4f} (limit {args.mean_threshold}), "
        f"mean_abs_diff_d4={overall_mean_d4:.4f}, "
        f"max_abs_diff={overall_max}, "
        f"changed_pixels_pct={overall_changed:.4f} (limit {args.changed_pct_threshold})"
    )
    print(
        f"worst frame: {worst[0]:06d} mean_abs_diff={worst[1]['mean_abs_diff']:.4f} "
        f"max_abs_diff={worst[1]['max_abs_diff']} "
        f"changed_pixels_pct={worst[1]['changed_pixels_pct']:.4f}"
    )
    print(f"verdict: {verdict}")
    print("=" * 78)

    if args.report:
        report_data = {
            "baseline": os.path.abspath(args.baseline_run_dir),
            "candidate": os.path.abspath(args.candidate_run_dir),
            "scenario": (manifest_a or {}).get("scenario"),
            "frames_compared": len(rows),
            "tolerance": args.tolerance,
            "mean_threshold": args.mean_threshold,
            "changed_pct_threshold": args.changed_pct_threshold,
            "summary": {
                "mean_abs_diff": overall_mean,
                "mean_abs_diff_d4": overall_mean_d4,
                "max_abs_diff": overall_max,
                "changed_pixels_pct": overall_changed,
                "worst_frame": worst[0],
                "worst_mean_abs_diff": worst[1]["mean_abs_diff"],
                "passed": passed,
            },
            "frames": [
                {"index": index, **row}
                for index, row in rows
            ],
        }
        if args.report.endswith(".json"):
            with open(args.report, "w", encoding="utf-8") as fh:
                json.dump(report_data, fh, indent=2)
        else:
            with open(args.report, "w", encoding="utf-8") as fh:
                fh.write(f"# QA frame comparison: {report_data['scenario']}\n\n")
                fh.write(f"- baseline: {report_data['baseline']}\n")
                fh.write(f"- candidate: {report_data['candidate']}\n")
                fh.write(f"- frames compared: {report_data['frames_compared']}\n")
                fh.write(f"- tolerance: {report_data['tolerance']}\n")
                fh.write(
                    f"- thresholds: mean_abs_diff <= {args.mean_threshold}, "
                    f"changed_pixels_pct <= {args.changed_pct_threshold}\n"
                )
                summary = report_data["summary"]
                fh.write(
                    f"- summary: mean_abs_diff={summary['mean_abs_diff']:.4f}, "
                    f"mean_abs_diff_d4={summary['mean_abs_diff_d4']:.4f}, "
                    f"max_abs_diff={summary['max_abs_diff']}, "
                    f"changed_pixels_pct={summary['changed_pixels_pct']:.4f}\n"
                )
                fh.write(
                    f"- worst frame: {summary['worst_frame']:06d} "
                    f"(mean_abs_diff={summary['worst_mean_abs_diff']:.4f})\n"
                )
                fh.write(f"- verdict: {'PASS' if summary['passed'] else 'FAIL'}\n\n")
                fh.write("| frame | mean_abs_diff | mean_abs_diff_d4 | max_abs_diff | changed_pixels_pct |\n")
                fh.write("|------:|--------------:|-----------------:|-------------:|-------------------:|\n")
                for frame in report_data["frames"]:
                    fh.write(
                        f"| {frame['index']:06d} | {frame['mean_abs_diff']:.4f} | "
                        f"{frame['mean_abs_diff_d4']:.4f} | {frame['max_abs_diff']} | "
                        f"{frame['changed_pixels_pct']:.4f} |\n"
                    )
        print(f"report written to {args.report}")

    return 0 if passed else 1


if __name__ == "__main__":
    sys.exit(main())
