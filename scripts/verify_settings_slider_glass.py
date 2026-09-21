#!/usr/bin/env python3
"""Verify the production slider lifecycle from GPU scenario output (Pillow)."""
import json
import sys
from pathlib import Path

from PIL import Image, ImageChops, ImageStat
from verify_qa_artifact import verify_manifest


def require(condition, message):
    if not condition:
        raise SystemExit(f"slider QA failed: {message}")


def verify(root):
    verify_manifest(root / "manifest.json")
    data = json.loads((root / "manifest.json").read_text(encoding="utf-8"))
    require(data["scenario"] == "settings-slider-glass", "wrong scenario")

    def frame(ms):
        return next(f for f in data["frames"] if f["elapsed_ms"] >= ms)

    # ALL[3] is AutoDarknessMax. Auto adaptation is off, so changing it cannot
    # alter the background and obscure tests of the thumb material itself.
    for ms, value, active in [
        (950, .25, True), (1500, .75, True), (1950, .75, False),
        (2200, .1, False), (2450, .1, False),
        (2850, 1., True), (3200, 1., False),
        (3500, 0., True), (3800, 0., False),
        (4050, .6, True), (4400, .6, False), (4650, .6, False),
        (4900, .2, True), (5400, .2, False), (6000, .2, False),
        (6400, .16, False), (6650, .16, False),
        (6900, .3, True), (7150, .3, False),
    ]:
        record = frame(ms)
        require(abs(record["settings_slider_values"][3] - value) < .015,
                f"value at {ms}ms: {record['settings_slider_values'][3]} != {value}")
        require((record["settings_slider_drag"] is not None) == active,
                f"drag latch at {ms}ms")
        require(len(record["settings_slider_lenses"]) == int(active),
                f"press-only lens at {ms}ms")
        require(not record["settings_toggle_lenses"], "slider activated a toggle lens")

    def image(ms):
        with Image.open(root / frame(ms)["file"]) as im:
            return im.convert("RGB")

    # Long hold reaches a bounded puff, and the thumb remains visible after
    # release. Sampling at the same knob center distinguishes clear vs solid.
    x, y, w, h = frame(1500)["settings_slider_lenses"][0]
    require(26 < w < 36 and abs(w - h) < .01, "unbounded or missing puff")
    center = (round(x + w / 2), round(y + h / 2 - 6))
    held, released = image(1500).getpixel(center), image(1950).getpixel(center)
    require(sum(released) > sum(held) + 12,
            f"held thumb is still a solid white knob: {held}, {released}")
    # No scene cache contamination away from the slider row.
    region = (450, 200, 950, 370)
    diff = ImageChops.difference(image(700).crop(region), image(1500).crop(region))
    require(max(ImageStat.Stat(diff).mean) < .5, "slider changed the surrounding panel")
    print(json.dumps({"passed": True, "frames": len(data["frames"]),
                      "held_rgb": held, "released_rgb": released,
                      "lens_size": [w, h]}, indent=2))


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit("usage: verify_settings_slider_glass.py <scenario-output-directory>")
    verify(Path(sys.argv[1]))
