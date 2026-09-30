#!/usr/bin/env python3
"""Validate a deterministic Launchpad GPU visual-QA artifact."""

from __future__ import annotations

import hashlib
import json
import struct
import sys
from pathlib import Path


PNG_SIGNATURE = b"\x89PNG\r\n\x1a\n"


def fail(message: str) -> None:
    raise SystemExit(f"visual QA verification failed: {message}")


def png_size(path: Path) -> tuple[int, int]:
    data = path.read_bytes()
    if len(data) < 24 or data[:8] != PNG_SIGNATURE or data[12:16] != b"IHDR":
        fail(f"{path} is not a valid PNG")
    return struct.unpack(">II", data[16:24])


def verify(root: Path, require_default_scenarios: bool = True) -> None:
    manifests = sorted(root.glob("*/manifest.json"))
    if not manifests:
        fail(f"expected at least one manifest below {root}")

    scenarios: set[str] = set()
    for manifest_path in manifests:
        scenarios.add(verify_manifest(manifest_path))

    if require_default_scenarios:
        expected = {
            "folder-interactions",
            "folder-creation",
            "context-menu-open",
            "context-menu-dismiss",
            "context-menu-trackpad-secondary-click",
        }
        missing = expected - scenarios
        if missing:
            fail(f"missing required scenarios: {', '.join(sorted(missing))}")


def verify_manifest(manifest_path: Path) -> str:
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    if manifest.get("completed") is not True:
        fail("scenario did not complete")

    viewport = tuple(manifest.get("viewport", ()))
    if len(viewport) != 2:
        fail("manifest viewport is missing")

    frames = manifest.get("frames", [])
    if len(frames) < 5:
        fail(f"expected at least 5 frames, found {len(frames)}")

    hashes: set[str] = set()
    for frame in frames:
        frame_path = manifest_path.parent / frame["file"]
        if not frame_path.is_file():
            fail(f"missing frame {frame_path}")
        if png_size(frame_path) != viewport:
            fail(f"{frame_path} does not match viewport {viewport}")
        hashes.add(hashlib.sha256(frame_path.read_bytes()).hexdigest())

    if len(hashes) < 3:
        fail(f"expected at least 3 visually distinct frames, found {len(hashes)}")

    scenario = manifest.get("scenario")
    if not isinstance(scenario, str) or not scenario:
        fail(f"{manifest_path} has no scenario name")
    required_states: dict[str, bool] = {}
    if scenario in {"folder-interactions", "folder-creation"}:
        required_states["folder open"] = any(frame.get("folder_open") for frame in frames)
    elif scenario in {
        "context-menu-open",
        "context-menu-dismiss",
        "context-menu-trackpad-secondary-click",
    }:
        assertions = manifest.get("context_menu_assertions")
        required_states["context menu assertions passed"] = bool(
            isinstance(assertions, dict) and assertions.get("passed") is True
        )
        required_states["context menu opening"] = any(
            frame.get("context_menu_phase") == "Opening" for frame in frames
        )
        if scenario in {
            "context-menu-open",
            "context-menu-trackpad-secondary-click",
        }:
            required_states["context menu open"] = any(
                frame.get("context_menu_phase") == "Open" for frame in frames
            )
        if scenario != "context-menu-open":
            required_states["context menu closing"] = any(
                frame.get("context_menu_phase") == "Closing" for frame in frames
            )
            required_states["context menu closed"] = any(
                frame.get("context_menu_phase") == "Closed" for frame in frames
            )
    if scenario == "folder-interactions":
        required_states["second folder page"] = any(
            frame.get("folder_page", 0) >= 1 for frame in frames
        )
    elif scenario == "folder-creation":
        required_states.update(
            {
                "two apps merged into folder": any(
                    frame.get("active_folder_child_count") == 2 for frame in frames
                ),
                "top-level items compacted": any(
                    frame.get("top_level_item_count") == 3 for frame in frames
                ),
                "later apps remain visible in model": any(
                    frame.get("folder_open")
                    and frame.get("top_level_item_count") == 3
                    for frame in frames
                ),
            }
        )
    elif scenario == "macos-initial-folder-layout-reset":
        counts: list[int] = []
        for frame in frames:
            count = frame.get("top_level_item_count")
            if count is not None and (not counts or counts[-1] != count):
                counts.append(count)
        required_states["classification, undo, reset, undo (39/217/39/217 items)"] = (
            counts == [39, 217, 39, 217]
        )
        required_states["development folder opens with fourteen children"] = any(
            frame.get("folder_open") and frame.get("active_folder_child_count") == 14
            for frame in frames
        )
        required_states["avatar folder opens with nine children"] = any(
            frame.get("folder_open") and frame.get("active_folder_child_count") == 9
            for frame in frames
        )
        required_states["second main page is reachable"] = any(
            frame.get("pager_position_x", 0) is not None
            and frame.get("pager_position_x", 0) < -1200
            and frame.get("pager_surface") == "main"
            and not frame.get("folder_open")
            for frame in frames
        )
        required_states["page gesture contract passed"] = (
            manifest.get("scroll_assertions", {}).get("passed") is True
        )
        direct_ids = {
            "app:mac:com.apple.AppStore", "app:mac:com.google.Chrome", "app:mac:com.apple.Safari",
            "app:mac:com.valvesoftware.steam", "app:mac:com.apple.systempreferences",
            "app:mac:com.apple.calculator",
        }
        required_states["exactly six reviewed apps and thirty-three folders"] = any(
            frame.get("top_level_item_count") == 39
            and set(i for i in frame.get("top_level_item_ids", []) if not i.startswith("folder:")) == direct_ids
            and len([i for i in frame.get("top_level_item_ids", []) if i.startswith("folder:")]) == 33
            for frame in frames
        )
        required_states["undo restores all 217 fixture IDs"] = any(
            frame.get("top_level_item_count") == 217
            and len(set(frame.get("top_level_item_ids", []))) == 217
            for frame in frames
        )
    missing = [name for name, present in required_states.items() if not present]
    if missing:
        fail(f"scenario never reached: {', '.join(missing)}")

    print(
        f"visual QA verified ({scenario}): {len(frames)} frames, "
        f"{len(hashes)} distinct images, viewport {viewport[0]}x{viewport[1]}"
    )
    return scenario


def main() -> None:
    if len(sys.argv) not in (2, 3) or (len(sys.argv) == 3 and sys.argv[2] != "--allow-partial"):
        fail("usage: verify_qa_artifact.py <qa-sequences-root> [--allow-partial]")
    verify(Path(sys.argv[1]), require_default_scenarios=len(sys.argv) == 2)


if __name__ == "__main__":
    main()
