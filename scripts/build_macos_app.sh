#!/usr/bin/env bash
# Build a launchable Launchpad.app bundle from the current source tree.
#
# Mirrors the "Package application bundle" step of
# .github/workflows/pr-macos-artifacts.yml so the local artifact matches the CI
# review bundle: the release binary, assets/macos/Info.plist, the Swift runtime
# dylibs bundled via swift-stdlib-tool, and an ad-hoc signature.
#
# Usage:
#   scripts/build_macos_app.sh            # release build (default)
#   scripts/build_macos_app.sh --debug    # dev profile
#   scripts/build_macos_app.sh --open     # build then open the app
#   scripts/build_macos_app.sh -- --features wheel-debug   # extra cargo flags
#
# The bundle is written to dist/Launchpad.app (relative to the repo root).
set -euo pipefail

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "build_macos_app.sh requires macOS" >&2
  exit 1
fi

# Match CI's deployment target so the local artifact runs on the same macOS
# versions as the review bundle and links the same Swift runtime subset.
export MACOSX_DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-14.0}"

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$repo_root"

bin_name="launchpad-windows"
app_name="Launchpad"
app_dir="dist/${app_name}.app"
profile="release"
do_open=0
extra_flags=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    --debug)
      profile="debug"
      ;;
    --release)
      profile="release"
      ;;
    --open)
      do_open=1
      ;;
    --)
      shift
      extra_flags+=("$@")
      set --
      ;;
    *)
      echo "Unknown option: $1" >&2
      exit 1
      ;;
  esac
  shift || true
done

if [[ "$profile" == "debug" ]]; then
  build_dir="target/debug"
else
  build_dir="target/release"
fi

extra_args=()
if [[ ${#extra_flags[@]} -gt 0 ]]; then
  extra_args=("${extra_flags[@]+"${extra_flags[@]}"}")
  echo "==> Building $bin_name ($profile) with flags: ${extra_flags[*]+"${extra_flags[*]}"}"
else
  echo "==> Building $bin_name ($profile)"
fi
if [[ "$profile" == "debug" ]]; then
  if [[ ${#extra_args[@]} -gt 0 ]]; then
    cargo build --bin "$bin_name" "${extra_args[@]}"
  else
    cargo build --bin "$bin_name"
  fi
else
  if [[ ${#extra_args[@]} -gt 0 ]]; then
    cargo build --release --locked --bin "$bin_name" "${extra_args[@]}"
  else
    cargo build --release --locked --bin "$bin_name"
  fi
fi

binary="$build_dir/$bin_name"
if [[ ! -x "$binary" ]]; then
  echo "Expected binary not found at $binary" >&2
  exit 1
fi

echo "==> Packaging $app_dir"
rm -rf "$app_dir"
contents="$app_dir/Contents"
mkdir -p "$contents/MacOS" "$contents/Frameworks" "$contents/Resources"
cp "$binary" "$contents/MacOS/$bin_name"
cp assets/macos/Info.plist "$contents/Info.plist"

echo "==> Bundling Swift runtime libraries"
# build.rs sets @executable_path/../Frameworks as an rpath, so the Swift dylibs
# copied here by swift-stdlib-tool are picked up at launch.
xcrun swift-stdlib-tool \
  --copy \
  --scan-executable "$contents/MacOS/$bin_name" \
  --destination "$contents/Frameworks" \
  --platform macosx \
  --strip-bitcode

echo "==> Ad-hoc signing"
find "$contents/Frameworks" -type f -name '*.dylib' -exec codesign --force --sign - {} \;
codesign --force --deep --sign - "$app_dir"
codesign --verify --deep --strict "$app_dir"

echo "==> Done: $app_dir"
if [[ "$do_open" -eq 1 ]]; then
  echo "==> Opening $app_dir"
  open "$app_dir"
fi
