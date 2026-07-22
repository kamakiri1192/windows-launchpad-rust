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
scenario_list="${SCENARIOS:-qa/folder_interactions.json qa/folder_creation.json}"
timestamp="$(date -u +%Y%m%dT%H%M%SZ)"
output_dir="${OUTPUT_DIR:-target/macos-profile-$mode-$timestamp}"

if [[ -n "${CAPTURE_SCALE:-}" ]]; then
  export LAUNCHPAD_MACOS_CAPTURE_SCALE="$CAPTURE_SCALE"
fi
capture_scale="${LAUNCHPAD_MACOS_CAPTURE_SCALE:-auto-logical-point}"

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
  sw_vers
  system_profiler SPHardwareDataType SPDisplaysDataType
} > "$output_dir/environment.txt"

if [[ "${SKIP_BUILD:-0}" != "1" ]]; then
  cargo build --release --locked --features gpu-profile
fi

binary="$repo_root/target/release/launchpad-windows"

case "$mode" in
  qa)
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
    python3 scripts/verify_qa_artifact.py "$qa_output"
    ;;
  live)
    if pgrep -x launchpad-windows >/dev/null; then
      echo "another launchpad-windows process is running" >&2
      exit 1
    fi
    mkdir -p "$output_dir/home"
    HOME="$output_dir/home" \
    LAUNCHPAD_DEBUG=1 \
    LAUNCHPAD_GPU_PROFILE="$output_dir/live.json" \
    WGPU_BACKEND=metal \
    RUST_LOG=warn \
      "$binary" > "$output_dir/live.log" 2>&1 &
    pid=$!
    trap 'kill -TERM "$pid" 2>/dev/null || true' EXIT INT TERM
    # The resident app intentionally hides when its first window loses focus.
    # Let discovery/icon work settle, then launch the same binary again: the
    # single-instance handoff summons the measured process through the same
    # path used by the global shortcut/menu-bar action.
    sleep "$warmup"
    HOME="$output_dir/home" "$binary" >/dev/null 2>&1 || true
    sleep 1
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
    trap - EXIT INT TERM
    ;;
  *)
    echo "usage: $0 [qa|live]" >&2
    exit 2
    ;;
esac

python3 scripts/summarize_macos_profile.py "$output_dir"
echo "reports: $output_dir"
