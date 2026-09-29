#!/usr/bin/env python3
"""Check qa/settings_toggle_glass.json GPU output (requires Pillow)."""

import json
import sys
from pathlib import Path

from PIL import Image, ImageChops, ImageStat

from verify_qa_artifact import verify_manifest


def require(condition, message):
    if not condition:
        raise SystemExit(f"settings glass regression: {message}")


def verify(root):
    verify_manifest(root / "manifest.json")
    manifest = json.loads((root / "manifest.json").read_text(encoding="utf-8"))
    require(manifest["scenario"] == "settings-toggle-glass", "wrong scenario")
    require(manifest["viewport"] == [1280, 800], "unexpected fixture viewport")

    def frame(ms):
        record = next(f for f in manifest["frames"] if f["elapsed_ms"] >= ms)
        with Image.open(root / record["file"]) as image:
            return record, image.convert("RGB")

    # Sample a clear portion of each capsule, away from text and gear ink.
    def bottom_sample(ms, index):
        record, image = frame(ms)
        rects = sorted(record["bottom_glass_rects"], key=lambda r: r[0])
        require(len(rects) > index, "missing bottom glass surface")
        x, y, width, height = rects[index]
        point = (round(x + width * 0.18), round(y + height * 0.5))
        return sum(image.getpixel(point)) / 3

    search_light, search_dark = bottom_sample(300, 0), bottom_sample(650, 0)
    gear_dark, gear_light = bottom_sample(1150, 1), bottom_sample(1600, 1)
    require(search_light > search_dark + 15, "search ignores darkness")
    require(gear_light > gear_dark + 15, "settings gear ignores darkness")

    for ms in [2350, 4300, 5500, 6200, 6850]:
        require(not frame(ms)[0]["settings_toggle_lenses"], f"idle lens at {ms}ms")
    for ms in [2700, 3150, 3650, 3990, 4650, 5200, 6550]:
        require(len(frame(ms)[0]["settings_toggle_lenses"]) == 1, f"missing held lens at {ms}ms")

    held, light_image = frame(3150)
    dark_held, dark_image = frame(3650)
    x, y, width, height = held["settings_toggle_lenses"][0]
    center = (round(x + width / 2), round(y + height / 2))
    light_color, dark_color = light_image.getpixel(center), dark_image.getpixel(center)
    require(light_color[1] > light_color[0] + 35 and light_color[1] > light_color[2] + 20,
            f"green track obscured by lens: {light_color}")
    require(max(abs(a - b) for a, b in zip(light_color, dark_color)) <= 2,
            "global darkness changed the lens interior")
    # Two separated points on the flat green track must remain the same color.
    # The settings layout can move the switch when its header or row spacing
    # changes. Sample the same track just left of the lens in this frame.
    uncovered_track = light_image.getpixel((round(x - 16), center[1]))
    require(max(abs(a - b) for a, b in zip(light_color, uncovered_track)) <= 3,
            f"lens adds a film/tint: {light_color} vs track {uncovered_track}")

    pulled, pulled_image = frame(3990)
    require(pulled["settings_toggle_lenses"][0][2] > dark_held["settings_toggle_lenses"][0][2] + 2,
            "overdrag did not extend the lens")
    # A missing GPU lens could otherwise pass the green-color check above.
    delta = ImageChops.difference(dark_image, pulled_image).crop((1070, 160, 1190, 220))
    changed = sum(max(delta.getpixel((x, y))) > 3
                  for y in range(delta.height) for x in range(delta.width))
    require(changed > 20, "deforming lens silhouette is invisible")

    # The post-track copy must not overwrite the cached pre-panel scene.
    before = frame(2350)[1].crop((440, 260, 1160, 720))
    after = frame(2750)[1].crop((440, 260, 1160, 720))
    panel_difference = max(ImageStat.Stat(ImageChops.difference(before, after)).mean)
    require(panel_difference < 0.5, "holding a toggle changed the surrounding panel")
    print(json.dumps({
        "passed": True,
        "search_light_dark": [search_light, search_dark],
        "gear_light_dark": [gear_light, gear_dark],
        "lens_rgb_light_dark": [light_color, dark_color],
        "overdrag_changed_pixels": changed,
        "panel_mean_difference": panel_difference,
    }, indent=2))


if __name__ == "__main__":
    if len(sys.argv) != 2:
        raise SystemExit("usage: verify_settings_toggle_glass.py <scenario-output-directory>")
    verify(Path(sys.argv[1]))
