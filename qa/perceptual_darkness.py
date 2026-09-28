#!/usr/bin/env python3
"""Reproducible CPU color-kernel comparison, NOT native GPU/Windows QA.

Requires Python 3.10+ and numpy. Pillow is optional (contact sheet only).
Run: python qa/perceptual_darkness.py --out target/perceptual-darkness
No desktop capture, user data, or external network access is performed.
"""
from __future__ import annotations

import argparse
import csv
import hashlib
import json
from pathlib import Path

import numpy as np

WEIGHTS = np.array([0.2126, 0.7152, 0.0722])
METHODS = ("current", "coverage_tuned", "mean_ceiling", "p75_ceiling", "local_shoulder")


def decode(c):
    c = np.maximum(c, 0.0)
    return np.where(c <= 0.04045, c / 12.92, ((c + 0.055) / 1.055) ** 2.4)


def encode(c):
    c = np.maximum(c, 0.0)
    return np.where(c <= 0.0031308, c * 12.92, 1.055 * c ** (1.0 / 2.4) - 0.055)


def smooth(a, b, x):
    t = np.clip((x - a) / (b - a), 0.0, 1.0)
    return t * t * (3.0 - 2.0 * t)


def auto_gain(y, maximum):
    cap = np.clip(maximum, 0.0, 1.0)
    if cap <= 0.0:
        return np.ones_like(y)
    if cap >= 1.0:
        return np.zeros_like(y)
    white = decode(1.0 - cap)
    return white / (white + (1.0 - white) * np.clip(y, 0.0, 1.0))


def local(rgb, maximum=0.16, manual=0.0):
    """Independent float64 reference; shader uses an equivalent scalar-power encode."""
    rgb = np.asarray(rgb, dtype=np.float64)
    m = np.clip(manual, 0.0, 1.0)
    mrgb = rgb * (1.0 - m)
    if maximum <= 0.0 or m >= 1.0:
        return mrgb.copy()
    linear = decode(rgb)
    target = (linear @ WEIGHTS) * auto_gain(linear @ WEIGHTS, maximum)
    mlinear = decode(mrgb) if m > 0.0 else linear
    my = mlinear @ WEIGHTS
    gain = np.divide(target, my, out=np.ones_like(my), where=my > 0.0)
    result = encode(mlinear * np.minimum(1.0, gain)[..., None])
    return np.where((my <= target)[..., None], mrgb, result)


def coverage(rgb, tuned=False):
    luma = rgb @ np.array([0.299, 0.587, 0.114])
    chroma = np.ptp(rgb, axis=-1)
    a, b = (0.65, 0.87) if tuned else (0.68, 0.90)
    score = smooth(a, b, luma) * (1.0 - smooth(0.10, 0.26, chroma))
    return float(np.mean(score))


def compare(rgb, method, maximum=0.16):
    """Steady-state algorithms at equal full-white attenuation.

    Ceiling competitors use linear-RGB gain with target Y=0.20. They are
    color-space-correct comparators, NOT a claim to execute PR #198's binary.
    Coverage uses all corpus pixels here to avoid handicapping it with sparse
    grid aliasing. The runtime baseline actually uses 256 texture samples.
    """
    if method in ("current", "coverage_tuned"):
        tuned = method == "coverage_tuned"
        a, b = (0.40, 0.70) if tuned else (0.55, 0.82)
        return rgb * (1.0 - maximum * smooth(a, b, coverage(rgb, tuned)))
    if method in ("mean_ceiling", "p75_ceiling"):
        linear = decode(rgb)
        y = linear @ WEIGHTS
        stat = float(np.mean(y) if method == "mean_ceiling" else np.quantile(y, 0.75))
        gain = np.clip(0.20 / max(stat, 1e-12), decode(1.0 - maximum), 1.0)
        return encode(linear * gain)
    if method == "local_shoulder":
        return local(rgb, maximum)
    raise ValueError(f"Unknown method: {method}")


def corpus():
    size = (64, 128, 3)
    colors = {
        "black": (0, 0, 0), "gray_10": (.1, .1, .1),
        "gray_50": (.5, .5, .5), "gray_80": (.8, .8, .8),
        "white": (1, 1, 1), "red": (1, 0, 0),
        "green": (0, 1, 0), "blue": (0, 0, 1),
        "yellow": (1, 1, 0), "cyan": (0, 1, 1), "magenta": (1, 0, 1),
    }
    result = {name: np.full(size, color, dtype=float) for name, color in colors.items()}
    for fraction in (.15, .24, .26, .50, .75):
        image = np.full(size, .10)
        image[:, :round(size[1] * fraction)] = 1.0
        result[f"white_patch_{int(fraction * 100)}"] = image
    result["neutral_ramp"] = np.broadcast_to(np.linspace(0, 1, size[1])[None, :, None], size)
    yy, xx = np.mgrid[:size[0], :size[1]]
    checker = ((xx // 4 + yy // 4) % 2).astype(float)
    result["checkerboard"] = np.repeat(checker[..., None], 3, axis=-1)
    rng = np.random.default_rng(20260928)
    result["random_rgb"] = rng.random(size)
    result["color_ramp"] = np.stack((xx / 127, yy / 63, 1 - xx / 127), axis=-1)
    return result


def old_temporal_steps(fps=60, maximum=.16):
    """Mirror main's full-rate compute order, including EMA and easing restarts."""
    state = dict(initialized=False, target=0., origin=0., started=0., filtered=0., last=0.)
    rows = []
    for frame in range(fps * 3):
        now = frame / fps
        value = 1.0 if fps <= frame < 2 * fps else 0.0
        duration = .18 if state["target"] > state["origin"] else .30
        displayed = state["origin"] + (state["target"] - state["origin"]) * smooth(
            0, 1, (now - state["started"]) / duration)
        elapsed = max(now - state["last"], 0.)
        alpha = 1 - np.exp(-elapsed / .12) if state["initialized"] else 1.
        filtered = state["filtered"] * (1 - alpha) + value * alpha
        target = float(maximum * smooth(.55, .82, filtered))
        if not state["initialized"] or abs(target - state["target"]) >= .001:
            state["origin"] = displayed if state["initialized"] else 0.
            state["target"] = target
            state["started"] = now
        state.update(initialized=True, filtered=filtered, last=now)
        rows.append((frame, float(displayed), value * maximum))
    rise = next(frame - fps for frame, old, _ in rows if frame >= fps and old >= maximum * .9)
    fall = next(frame - 2 * fps for frame, old, _ in rows if frame >= 2 * fps and old <= maximum * .1)
    return rows, rise, fall


def invariants():
    rng = np.random.default_rng(20260928)
    rgb = rng.random((20000, 3))
    ramp = np.repeat(np.linspace(0, 1, 4097)[:, None], 3, axis=1)
    caps = [0., .01, .16, .42, .85, .999, 1., 2.]
    manual_values = [0., .1, .3, .9, 1.]
    checks = 0
    for cap in caps:
        out = local(ramp, cap)
        assert np.all(np.diff(out[:, 0]) >= -1e-12)
        assert np.max(np.abs(local(np.ones((1, 3)), cap) - (1 - np.clip(cap, 0, 1)))) < 1e-12
        for manual in manual_values:
            out = local(rgb, cap, manual)
            assert np.isfinite(out).all()
            assert out.min() >= -1e-12
            assert np.all(out <= rgb + 1e-12)
            assert np.all(decode(out) @ WEIGHTS <= decode(rgb * (1 - manual)) @ WEIGHTS + 1e-12)
            assert np.array_equal(local(rgb, 0, manual), rgb * (1 - manual))
            checks += rgb.shape[0]
    lin = decode(rgb)
    corrected = decode(local(rgb))
    hue_error = float(np.max(np.abs(lin / lin.sum(axis=1)[:, None] - corrected / corrected.sum(axis=1)[:, None])))
    assert hue_error < 1e-12
    # Distant pixels may change; an unchanged pixel must not pump in response.
    contexts = [np.tile([.5, .5, .5], (64, 1)), np.tile([1., 1., 1.], (64, 1))]
    for context in contexts:
        context[0] = [.2, .4, .7]
    assert np.array_equal(local(contexts[0])[0], local(contexts[1])[0])
    return {"random_color_parameter_cases": checks, "neutral_ramp_samples_per_cap": len(ramp),
            "caps": caps, "manual_values": manual_values,
            "max_linear_chromaticity_error_float64": hue_error,
            "scope": "CPU float64 color kernel; not a GPU tolerance or perceptual study"}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", type=Path, default=Path("target/perceptual-darkness"))
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    images = corpus()
    checks = invariants()
    rows = []
    for name, rgb in images.items():
        for method in METHODS:
            out = compare(rgb, method)
            y0, y1 = decode(rgb) @ WEIGHTS, decode(out) @ WEIGHTS
            rows.append({"background": name, "method": method,
                         "input_mean_Y": float(y0.mean()), "output_mean_Y": float(y1.mean()),
                         "mean_absolute_srgb_change": float(np.abs(rgb-out).mean()),
                         "dark_pixel_mean_change": float(np.abs(rgb-out)[y0 < .02].mean()) if (y0 < .02).any() else 0.})
    with (args.out / "comparison.csv").open("w", newline="", encoding="utf-8") as f:
        writer = csv.DictWriter(f, fieldnames=list(rows[0]))
        writer.writeheader()
        writer.writerows(rows)
    temporal, rise, fall = old_temporal_steps()
    with (args.out / "temporal.csv").open("w", newline="", encoding="utf-8") as f:
        writer = csv.writer(f)
        writer.writerow(["frame_at_60Hz", "old_compute_displayed_darkness", "new_local_white_equivalent_darkness"])
        writer.writerows(temporal)
    shader = Path(__file__).resolve().parents[1] / "assets/shaders/liquid_glass_final.wgsl"
    data = {"status": "CPU_REFERENCE_VERIFIED_NATIVE_GPU_UNMEASURED", "corpus_images": len(images),
            "methods": list(METHODS), "comparisons": len(rows), "invariants": checks,
            "shader_sha256": hashlib.sha256(shader.read_bytes()).hexdigest(),
            "temporal_simulation_60Hz": {"old_rise_to_90_percent_frames": rise,
                                         "old_fall_to_10_percent_frames": fall,
                                         "new_history_latency_frames": 0},
            "not_measured": ["WGSL execution", "GPU milliseconds", "Windows capture-to-present latency",
                             "native screenshots", "human perceptual preference", "final text contrast"]}
    try:
        from PIL import Image, ImageDraw, ImageFont
        font = ImageFont.load_default(size=14)
        names = ["white", "yellow", "green", "blue", "gray_10", "white_patch_15", "white_patch_50", "neutral_ramp", "color_ramp"]
        width, height, label = 156, 78, 22
        sheet = Image.new("RGB", (width*6, 72+(height+label)*len(names)), "#242424")
        draw = ImageDraw.Draw(sheet)
        draw.text((8, 8), "CPU color-kernel comparison | cap=0.16 | NOT app screenshots", font=font, fill="white")
        for col, method in enumerate(("input",)+METHODS):
            draw.text((col*width+4, 42), method, font=font, fill="white")
        for row, name in enumerate(names):
            for col, method in enumerate(("input",)+METHODS):
                rgb = images[name] if method == "input" else compare(images[name], method)
                tile = Image.fromarray(np.rint(np.clip(rgb, 0, 1)*255).astype("uint8")).resize((width-6, height))
                origin = (col*width+3, 72+row*(height+label))
                sheet.paste(tile, origin)
                draw.text((origin[0]+2, origin[1]+height+2), name, font=font, fill="white")
        sheet.save(args.out / "comparison.png")
    except ImportError:
        data["contact_sheet"] = "Pillow not installed; numerical validation is unaffected"
    (args.out / "results.json").write_text(json.dumps(data, indent=2)+"\n", encoding="utf-8")
    print(json.dumps(data, indent=2))


if __name__ == "__main__":
    main()
