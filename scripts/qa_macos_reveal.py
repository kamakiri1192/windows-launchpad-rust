#!/usr/bin/env python3
"""Record real WindowServer composition, including live backdrop capture.

Quit the existing launcher first. Requires Screen Recording for the recorder
and launcher, and Accessibility for the Escape generator. Uses normal native
hide/summon; does not change animation duration or enable headless QA.
"""
import argparse
import json
import os
from pathlib import Path
import subprocess
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--app", type=Path, default=Path("dist/Launchpad.app"))
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--display", type=int, default=1)
    parser.add_argument("--repeats", type=int, default=3)
    parser.add_argument("--dynamic", action="store_true")
    parser.add_argument("--fallback", action="store_true",
                        help="disable capture to exercise the existing fallback material")
    args = parser.parse_args()
    if args.repeats < 1 or args.display < 1:
        parser.error("--repeats and --display must be positive")
    bundle = args.app.resolve()
    exe = bundle / "Contents/MacOS/launchpad-windows"
    if not exe.is_file():
        parser.error(f"missing executable: {exe}")
    processes = subprocess.check_output(["ps", "-axo", "comm="], text=True)
    if any(Path(line.strip()).name == "launchpad-windows" for line in processes.splitlines()):
        parser.error("quit the existing Launchpad before starting native reveal QA")
    out = args.output.resolve()
    out.mkdir(parents=True, exist_ok=False)
    helper = out / "reveal-fixture"
    subprocess.run(["swiftc", str(Path(__file__).with_name("macos_reveal_fixture.swift")),
                    "-o", str(helper)], check=True)
    debug = Path.home() / "Library/Logs/Launchpad/debug.log"
    env = os.environ.copy()
    env.update(LAUNCHPAD_DEBUG="1", LAUNCHPAD_ALLOW_SCREENSHOT="1",
               LAUNCHPAD_PROFILE_KEEP_VISIBLE="1", LAUNCHPAD_QA_SHOT_FILE=str(out / "trigger"))
    for key in ["LAUNCHPAD_QA_SCENARIO", "LAUNCHPAD_QA_HEADLESS"]:
        env.pop(key, None)
    if args.fallback:
        env["LAUNCHPAD_NO_BACKDROP_CAPTURE"] = "1"
    else:
        env.pop("LAUNCHPAD_NO_BACKDROP_CAPTURE", None)
    summon_env = env.copy()
    # A second process initializes the logger before sending its summon IPC.
    # Disable its logger so it does not truncate the resident process's log.
    summon_env.pop("LAUNCHPAD_DEBUG", None)
    children = []
    started = False
    events = []
    try:
        children.append(subprocess.Popen([str(helper)] + (["--dynamic"] if args.dynamic else [])))
        time.sleep(1)
        with (out / "app.log").open("w") as log:
            launcher = subprocess.Popen([str(exe)], env=env, stdout=log, stderr=log)
            children.append(launcher)
            started = True
            time.sleep(4)
            if launcher.poll() is not None:
                raise RuntimeError("launcher exited; see app.log")
            subprocess.run([str(exe)], env=summon_env, check=True)
            time.sleep(1)
            (out / "trigger").write_text(str(out / "settled.png"))
            time.sleep(0.5)
            duration = 4 + args.repeats * 3
            with (out / "record.log").open("w") as record_log:
                recorder = subprocess.Popen(["/usr/sbin/screencapture", "-x", "-v", "-V",
                    str(duration), "-D", str(args.display), str(out / "native.mov")],
                    stdout=record_log, stderr=record_log)
                children.append(recorder)
                time.sleep(1)
                for index in range(args.repeats):
                    subprocess.run([str(helper), "--escape"], check=True)
                    time.sleep(0.8)
                    events.append({"iteration": index, "summon_unix_seconds": time.time()})
                    subprocess.run([str(exe)], env=summon_env, check=True, timeout=5)
                    time.sleep(1.5)
                if recorder.wait(timeout=duration + 10) != 0:
                    raise RuntimeError("screen recorder failed; see record.log")
            trace = debug.read_text()
            (out / "debug.log").write_text(trace)
            (out / "events.json").write_text(json.dumps(events, indent=2))
            observed = trace.count("summon: preparing window (visible was false)")
            if observed != args.repeats:
                raise RuntimeError(f"expected {args.repeats} real reveals, observed {observed}; "
                                   "check focus/input permission and avoid interacting during QA")
            capture_log = (out / "app.log").read_text()
            if not args.fallback and ("platform backdrop ready" not in capture_log
                                     or "IOSurface -> persistent GPU texture" not in capture_log):
                raise RuntimeError("live backdrop capture unavailable; see app.log")
            if args.fallback and "fallback" not in capture_log.lower():
                raise RuntimeError("fallback was not observed; see app.log")
            print(f"Recorded {observed} real reveals: {out / 'native.mov'}")
    finally:
        for child in reversed(children):
            if child.poll() is None:
                child.terminate()
                try:
                    child.wait(timeout=4)
                except subprocess.TimeoutExpired:
                    child.kill()
                    child.wait()
        if started:
            subprocess.run(["open", str(bundle)], check=True)


if __name__ == "__main__":
    main()
