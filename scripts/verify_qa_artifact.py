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
    if len(manifests) != 1:
        fail(f"expected exactly one manifest below {root}, found {len(manifests)}")

    manifest_path = manifests[0]
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

    required_states = {
        "folder open": any(frame.get("folder_open") for frame in frames),
        "second folder page": any(frame.get("folder_page", 0) >= 1 for frame in frames),
    }
    missing = [name for name, present in required_states.items() if not present]
    if missing:
        fail(f"scenario never reached: {', '.join(missing)}")

    print(
        f"visual QA verified: {len(frames)} frames, "
        f"{len(hashes)} distinct images, viewport {viewport[0]}x{viewport[1]}"
    )


def main() -> None:
    if len(sys.argv) != 2:
        fail("usage: verify_qa_artifact.py <qa-sequences-root>")
    verify(Path(sys.argv[1]))


if __name__ == "__main__":
    main()
