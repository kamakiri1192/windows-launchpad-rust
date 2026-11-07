#!/usr/bin/env python3
"""Verify rendered label motion, not just edit-state flags (Pillow + numpy).

Run on qa/edit_label_wiggle.json output. --frames-dir substitutes another
capture's PNGs, so the same expected motion can reject pre-fix static labels.
The idle glyph image is transformed using the icon's recorded pivot/phase;
we compare actual white ink with both moving and stationary predictions.
"""

import argparse
import json
import math
from pathlib import Path

import numpy as np
from PIL import Image, ImageFilter

from verify_qa_artifact import verify_manifest


def require(condition, message):
    if not condition:
        raise SystemExit(f"label wiggle regression: {message}")


def ink(path, box):
    with Image.open(path) as image:
        rgb = np.asarray(image.convert("RGB").crop(box), dtype=np.float32)
    # Neutral white glyphs; reject colored tiles and dim glass/shadows.
    white = np.clip((rgb.min(axis=2) - 70.0) / 185.0, 0, 1)
    white[rgb.max(axis=2) - rgb.min(axis=2) > 35] = 0
    return Image.fromarray(np.uint8(white * 255))


def similarity(a, b):
    # Mild filtering tolerates the second resampling of an idle screenshot.
    a = np.asarray(a.filter(ImageFilter.GaussianBlur(0.5)), dtype=np.float64)
    b = np.asarray(b.filter(ImageFilter.GaussianBlur(0.5)), dtype=np.float64)
    return float(np.sum(a * b) / max(1, np.sqrt(np.sum(a * a) * np.sum(b * b))))


def predict(template, box, reference, label, clock, source_box=None):
    phase, lift, scale, flags = label["motion"]
    flags = int(flags)
    wave = math.sin((clock + phase) * 8) if flags & 1 else 0
    angle = wave * 0.06
    scale = scale if flags & 1 else 1
    if not flags & 1 or (label["lane"] == "grid" and flags & 8):
        lift = 0
    c, s = math.cos(angle) / scale, math.sin(angle) / scale
    px, py = label["pivot"]
    rx, ry = reference["pivot"]
    py -= abs(wave) * 2 + lift
    # Inverse affine: destination world -> idle template world -> crop.
    x, y = box[:2]
    sx, sy = (source_box or box)[:2]
    matrix = (c, s, rx + c * (x - px) + s * (y - py) - sx,
              -s, c, ry - s * (x - px) + c * (y - py) - sy)
    size = (box[2] - box[0], box[3] - box[1])
    return template.transform(size, Image.Transform.AFFINE, matrix,
                              resample=Image.Resampling.BILINEAR), angle


def verify(root, frames_dir):
    verify_manifest(root / "manifest.json")
    data = json.loads((root / "manifest.json").read_text())
    require(data["scenario"].startswith("edit-label-wiggle"), "wrong scenario")
    frames = data["frames"]
    results = []
    # Long stable holds exclude spring settling, dragging and folder opening.
    for lane, idle_ms, start, end, exit_ms in [
        ("grid", 300, 1500, 2600, 3200),
        ("folder", 4200, 5500, 6500, 7050),
    ]:
        reference_frame = next(f for f in frames if f["elapsed_ms"] >= idle_ms)
        exit_frame = next(f for f in frames if f["elapsed_ms"] >= exit_ms)
        require(not reference_frame["editing"] and not exit_frame["editing"],
                f"{lane}: missing idle/edit-exit samples")
        references = [r for r in reference_frame["labels"] if r["lane"] == lane]
        require(len(references) >= 3, f"{lane}: missing app/folder labels")
        for reference in references:
            x, y, w, h = reference["bounds"]
            # Below-icon ink only, with room for the complete rotated label.
            box = (math.floor(x - 12), math.floor(y - 12),
                   math.ceil(x + w + 12), math.ceil(y + h + 12))
            template = ink(frames_dir / reference_frame["file"], box)
            require(np.asarray(template).sum() > 1000, f"{reference['id']}: no ink")
            moving, stationary, angles = [], [], []
            for frame in frames:
                if not start <= frame["elapsed_ms"] <= end or frame["index"] % 3:
                    continue
                label = next(r for r in frame["labels"] if r["id"] == reference["id"])
                require(int(label["motion"][3]) & 1, f"{label['id']}: not wiggling")
                require(not int(label["motion"][3]) & 2, "fixture still dragging")
                expected, angle = predict(template, box, reference, label,
                                          frame["animation_time_s"])
                actual = ink(frames_dir / frame["file"], box)
                # At the extrema, static text cannot resemble moving text.
                if abs(angle) > 0.045:
                    moving.append(similarity(actual, expected))
                    stationary.append(similarity(actual, template))
                angles.append(angle)
            require(len(moving) >= 5 and min(angles) < -0.045 and max(angles) > 0.045,
                    f"{reference['id']}: insufficient motion coverage")
            stopped = similarity(ink(frames_dir / exit_frame["file"], box), template)
            score, static_score = float(np.mean(moving)), float(np.mean(stationary))
            result = dict(lane=lane, id=reference["id"], samples=len(moving),
                          moving_similarity=score, stationary_similarity=static_score,
                          exit_similarity=stopped)
            results.append(result)
            require(score > 0.85 and score > static_score + 0.12,
                    f"{reference['id']}: rendered ink does not follow icon: {result}")
            require(stopped > 0.97, f"{reference['id']}: label did not return to idle")
    # The held child crosses the folder's right scissor edge. Check the full
    # label at the pointer, so the pre-overlay implementation fails here.
    reference_frame = next(f for f in frames if f["elapsed_ms"] >= 4200)
    drag_scores = []
    for frame in frames:
        if not 7750 <= frame["elapsed_ms"] <= 8100 or frame["index"] % 3:
            continue
        label = next((r for r in frame["labels"]
                      if r["lane"] == "folder" and int(r["motion"][3]) & 2), None)
        require(label is not None, "missing folder edge drag")
        reference = next(r for r in reference_frame["labels"] if r["id"] == label["id"])
        rx, ry, w, h = reference["bounds"]
        source_box = (math.floor(rx - 3), math.floor(ry - 3),
                      math.ceil(rx + w + 3), math.ceil(ry + h + 3))
        template = ink(frames_dir / reference_frame["file"], source_box)
        phase, lift, scale, _ = label["motion"]
        wave = math.sin((frame["animation_time_s"] + phase) * 8)
        c, s = math.cos(wave * 0.06) * scale, math.sin(wave * 0.06) * scale
        corners = []
        for x in [rx, rx + w]:
            for y in [ry, ry + h]:
                dx, dy = x - reference["pivot"][0], y - reference["pivot"][1]
                corners.append((label["pivot"][0] + c * dx - s * dy,
                                label["pivot"][1] + s * dx + c * dy - abs(wave) * 2 - lift))
        box = (math.floor(min(p[0] for p in corners) - 3),
               math.floor(min(p[1] for p in corners) - 3),
               math.ceil(max(p[0] for p in corners) + 3),
               math.ceil(max(p[1] for p in corners) + 3))
        expected, _ = predict(template, box, reference, label,
                              frame["animation_time_s"], source_box)
        drag_scores.append(similarity(ink(frames_dir / frame["file"], box), expected))
    require(len(drag_scores) >= 5, "insufficient edge-drag frames")
    require(min(drag_scores) > 0.85, f"folder edge clipped the dragged label: {drag_scores}")
    print(json.dumps({"passed": True, "labels": results,
                      "edge_drag_similarity_min": min(drag_scores)}, ensure_ascii=False, indent=2))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=Path)
    parser.add_argument("--frames-dir", type=Path)
    args = parser.parse_args()
    verify(args.root, args.frames_dir or args.root)
