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


def verify(root: Path) -> None:
    manifests = sorted(root.glob("*/manifest.json"))
    if not manifests:
        fail(f"expected at least one manifest below {root}")

    scenarios: set[str] = set()
    for manifest_path in manifests:
        scenarios.add(verify_manifest(manifest_path))

    expected = {"folder-interactions", "folder-creation"}
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
    required_states = {"folder open": any(frame.get("folder_open") for frame in frames)}
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
    missing = [name for name, present in required_states.items() if not present]
    if missing:
        fail(f"scenario never reached: {', '.join(missing)}")

    print(
        f"visual QA verified ({scenario}): {len(frames)} frames, "
        f"{len(hashes)} distinct images, viewport {viewport[0]}x{viewport[1]}"
    )
    return scenario


def main() -> None:
    if len(sys.argv) != 2:
        fail("usage: verify_qa_artifact.py <qa-sequences-root>")
    verify(Path(sys.argv[1]))


if __name__ == "__main__":
    main()
