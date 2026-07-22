#!/usr/bin/env bash
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "profile_macos.sh requires macOS" >&2
  exit 1
fi

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo_root"

mode="${1:-qa}"
runs="${RUNS:-3}"
duration="${DURATION_SECONDS:-20}"
warmup="${WARMUP_SECONDS:-8}"
custom_scenarios=0
if [[ -n "${SCENARIOS+x}" ]]; then
  custom_scenarios=1
  scenario_list="$SCENARIOS"
elif [[ "$mode" == "folder-scroll" ]]; then
  scenario_list="qa/folder_page_scroll.json"
else
  scenario_list="qa/folder_interactions.json qa/folder_creation.json"
fi
if [[ -n "${ANIMATED_BACKDROP:-}" ]]; then
  animated_backdrop="$ANIMATED_BACKDROP"
elif [[ "$mode" == "live" || "$mode" == "gpu" || "$mode" == "scroll" || "$mode" == "scroll-gpu" || "$mode" == "edit" || "$mode" == "edit-gpu" ]]; then
  animated_backdrop=1
else
  animated_backdrop=0
fi
timestamp="$(date -u +%Y%m%dT%H%M%SZ)"
output_dir="${OUTPUT_DIR:-target/macos-profile-$mode-$timestamp}"

if [[ -n "${CAPTURE_SCALE:-}" ]]; then
  export LAUNCHPAD_MACOS_CAPTURE_SCALE="$CAPTURE_SCALE"
fi
capture_scale="${LAUNCHPAD_MACOS_CAPTURE_SCALE:-auto-from-display-scale}"

if [[ "$animated_backdrop" != "0" && "$animated_backdrop" != "1" ]]; then
  echo "ANIMATED_BACKDROP must be 0 or 1" >&2
  exit 2
fi

if [[ -e "$output_dir" ]]; then
  echo "output already exists: $output_dir" >&2
  exit 1
fi
mkdir -p "$output_dir"
output_dir="$(cd "$output_dir" && pwd)"

{
  echo "commit=$(git rev-parse HEAD)"
  echo "mode=$mode"
  echo "rustc=$(rustc --version)"
  echo "cargo=$(cargo --version)"
  echo "arch=$(uname -m)"
  echo "macos_capture_scale=$capture_scale"
  echo "animated_backdrop=$animated_backdrop"
  echo "warmup_seconds=$warmup"
  echo "duration_seconds=$duration"
  sw_vers
  system_profiler SPHardwareDataType SPDisplaysDataType
} > "$output_dir/environment.txt"

if [[ "${SKIP_BUILD:-0}" != "1" ]]; then
  if [[ "$mode" == "qa" || "$mode" == "folder-scroll" || "$mode" == "gpu" || "$mode" == "scroll-gpu" || "$mode" == "edit-gpu" ]]; then
    cargo build --release --locked --features gpu-profile
  else
    cargo build --release --locked
  fi
fi

binary="$repo_root/target/release/launchpad-windows"

case "$mode" in
  qa|folder-scroll)
    qa_output="$output_dir/qa-sequences"
    read -r -a scenarios <<< "$scenario_list"
    for scenario in "${scenarios[@]}"; do
      scenario_path="$repo_root/$scenario"
      scenario_key="$(basename "$scenario" .json)"
      scenario_copy="$output_dir/scenario-$scenario_key.json"
      python3 - "$scenario_path" "$scenario_copy" "$qa_output" <<'PY'
import json, pathlib, sys
source, destination, output = map(pathlib.Path, sys.argv[1:])
data = json.loads(source.read_text(encoding="utf-8"))
data["output_dir"] = str(output)
destination.write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
PY
      for run in $(seq 1 "$runs"); do
        mkdir -p "$output_dir/home-$scenario_key-$run"
        /usr/bin/time -p env \
          HOME="$output_dir/home-$scenario_key-$run" \
          LAUNCHPAD_DEBUG=1 \
          LAUNCHPAD_QA_HEADLESS=1 \
          LAUNCHPAD_QA_SCENARIO="$scenario_copy" \
          LAUNCHPAD_GPU_PROFILE="$output_dir/qa-$scenario_key-$run.json" \
          WGPU_BACKEND=metal \
          RUST_LOG=warn \
          "$binary" > "$output_dir/qa-$scenario_key-$run.log" 2>&1
      done
    done
    if [[ "$mode" == "folder-scroll" || "$custom_scenarios" == "1" ]]; then
      python3 scripts/verify_qa_artifact.py "$qa_output" --allow-partial
    else
      python3 scripts/verify_qa_artifact.py "$qa_output"
    fi
    ;;
  live|gpu|scroll|scroll-gpu|edit|edit-gpu)
    if pgrep -x launchpad-windows >/dev/null; then
      echo "another launchpad-windows process is running" >&2
      exit 1
    fi
    pid=""
    backdrop_pid=""
    cleanup() {
      if [[ -n "$pid" ]]; then
        kill -TERM "$pid" 2>/dev/null || true
      fi
      if [[ -n "$backdrop_pid" ]]; then
        kill -TERM "$backdrop_pid" 2>/dev/null || true
      fi
    }
    trap cleanup EXIT INT TERM

    if [[ "$animated_backdrop" == "1" ]]; then
      backdrop_binary="$output_dir/macos-animated-backdrop"
      xcrun swiftc -O scripts/macos_animated_backdrop.swift \
        -o "$backdrop_binary" -framework AppKit
      "$backdrop_binary" > "$output_dir/animated-backdrop.log" 2>&1 &
      backdrop_pid=$!
      sleep 1
    fi

    mkdir -p "$output_dir/home"
    profile_scroll=0
    if [[ "$mode" == "scroll" || "$mode" == "scroll-gpu" ]]; then
      profile_scroll=1
    fi
    profile_edit=0
    if [[ "$mode" == "edit" || "$mode" == "edit-gpu" ]]; then
      profile_edit=1
    fi
    if [[ "$mode" == "gpu" || "$mode" == "scroll-gpu" || "$mode" == "edit-gpu" ]]; then
      env \
        HOME="$output_dir/home" \
        LAUNCHPAD_DEBUG=1 \
        LAUNCHPAD_PROFILE_KEEP_VISIBLE=1 \
        LAUNCHPAD_PROFILE_SCROLL="$profile_scroll" \
        LAUNCHPAD_PROFILE_EDIT="$profile_edit" \
        LAUNCHPAD_GPU_PROFILE="$output_dir/gpu.json" \
        WGPU_BACKEND=metal \
        RUST_LOG=warn \
        "$binary" > "$output_dir/$mode.log" 2>&1 &
    else
      env \
        HOME="$output_dir/home" \
        LAUNCHPAD_DEBUG=1 \
        LAUNCHPAD_PROFILE_KEEP_VISIBLE=1 \
        LAUNCHPAD_PROFILE_SCROLL="$profile_scroll" \
        LAUNCHPAD_PROFILE_EDIT="$profile_edit" \
        WGPU_BACKEND=metal \
        RUST_LOG=warn \
        "$binary" > "$output_dir/$mode.log" 2>&1 &
    fi
    pid=$!
    # Let discovery and icon work settle, then exercise the same single-
    # instance summon path used by the menu-bar item and global shortcut.
    sleep "$warmup"
    HOME="$output_dir/home" "$binary" >/dev/null 2>&1 || true
    sleep 1
    python3 - "$output_dir/$mode.log" "$output_dir/runtime-warmup-counts.json" <<'PY'
import json, pathlib, sys

log_path, output_path = map(pathlib.Path, sys.argv[1:])
lines = log_path.read_text(encoding="utf-8", errors="replace").splitlines()
counts = {
    "macos_capture": sum("macOS capture stats:" in line for line in lines),
    "liquid_glass": sum("liquid glass stats:" in line for line in lines),
}
output_path.write_text(
    json.dumps({log_path.name: counts}, indent=2) + "\n",
    encoding="utf-8",
)
PY
    echo "elapsed_seconds,cpu_percent,rss_kb" > "$output_dir/process.csv"
    started=$SECONDS
    while kill -0 "$pid" 2>/dev/null && (( SECONDS - started < duration )); do
      values="$(ps -p "$pid" -o %cpu= -o rss= | xargs)"
      if [[ -n "$values" ]]; then
        echo "$((SECONDS - started)),${values/ /,}" >> "$output_dir/process.csv"
      fi
      sleep 1
    done
    kill -TERM "$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
    pid=""
    if [[ -n "$backdrop_pid" ]]; then
      kill -TERM "$backdrop_pid" 2>/dev/null || true
      wait "$backdrop_pid" 2>/dev/null || true
      backdrop_pid=""
    fi
    trap - EXIT INT TERM
    ;;
  *)
    echo "usage: $0 [qa|folder-scroll|live|gpu|scroll|scroll-gpu|edit|edit-gpu]" >&2
    exit 2
    ;;
esac

python3 scripts/summarize_macos_profile.py "$output_dir"
echo "reports: $output_dir"
