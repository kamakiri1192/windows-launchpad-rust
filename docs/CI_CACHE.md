# CI cache strategy

The current CI cache strategy uses GitHub Actions cache only. R2-backed
`sccache` is not configured in the workflows.

There are two cache layers:

1. `actions/cache` stores the Rust `target/` directory for debug and release
   jobs.
2. `Swatinem/rust-cache` stores Cargo registry and git source caches, with
   `cache-targets: false` so it does not also cache `target/`.

The goal is to keep repeated CI runs fast without maintaining external cache
infrastructure.

## Core idea

`main-ci.yml` is the cache warmer.

Every push to `main` runs both cache-producing families:

- `debug-checks` warms `windows-target-debug-...`
- `release-build` warms `windows-target-release-...`

The PR and release workflows then use byte-identical cache keys so they can
restore the cache already produced on `main`:

- `pr-ci.yml` restores the debug cache warmed by `main-ci.yml`.
- `pr-binary.yml` restores the release cache warmed by `main-ci.yml`.
- `release-assets.yml` restores the release cache warmed by `main-ci.yml`.

Because `main` is the default branch, its GitHub Actions cache entries are
available to pull request workflows. That is the main performance trick: after a
change lands on `main`, future PR checks and review-binary builds start from a
warm debug or release `target/` cache instead of rebuilding the world from a
cold checkout.

When a PR changes Rust sources, shaders, or build inputs, the exact hash may be
new. In that case the workflow still falls back to the nearest cache for the
same Rust version, target, and profile family, so unchanged dependencies and
older build artifacts can still be reused.

## Workflows

These workflows share the same cache pattern:

- `.github/workflows/pr-ci.yml`
- `.github/workflows/main-ci.yml`
- `.github/workflows/pr-binary.yml`
- `.github/workflows/release-assets.yml`

Debug checks use the `windows-target-debug-...` cache family:

- `pr-ci.yml`
- `main-ci.yml` job `debug-checks`

Release binary builds use the `windows-target-release-...` cache family:

- `main-ci.yml` job `release-build`
- `pr-binary.yml`
- `release-assets.yml`

The cache keys are intentionally byte-identical between matching workflow
families. The `main-ci.yml` jobs are the canonical warmers; PR and release jobs
are the consumers.

## Target cache key

The `target/` cache key includes:

- profile family: `debug` or `release`
- target triple: `x86_64-pc-windows-msvc`
- Rust version: `1.89.0`
- hash of build inputs:
  - `Cargo.toml`
  - `Cargo.lock`
  - `build.rs`
  - `src/**/*.rs`
  - `src/**/*.wgsl`
  - `assets/shaders/**/*.wgsl`

Example key shape:

```text
windows-target-debug-x86_64-pc-windows-msvc-rust-1.89.0-${hashFiles(...)}
windows-target-release-x86_64-pc-windows-msvc-rust-1.89.0-${hashFiles(...)}
```

The restore key omits only the input hash:

```text
windows-target-debug-x86_64-pc-windows-msvc-rust-1.89.0-
windows-target-release-x86_64-pc-windows-msvc-rust-1.89.0-
```

That lets GitHub restore the nearest older cache for the same Rust version,
target, and profile family when the exact source hash has not been cached yet.

## Cache flow

Each job follows the same order:

1. Check out the repository.
2. Install Rust `1.89.0`.
3. Restore the GitHub `target/` cache.
4. Restore Cargo registry and git source caches through `Swatinem/rust-cache`.
5. Run the job's Cargo commands.
6. If the job succeeded and the initial `target/` restore was not an exact hit,
   do a lookup-only restore for the primary key.
7. Save `target/` under the primary key only when that key still does not exist.

The lookup-only step avoids most duplicate-save failures caused by another job
or workflow already creating the immutable GitHub cache entry.

## Cargo source cache

`Swatinem/rust-cache@v2` is configured with:

```yaml
cache-targets: false
```

This keeps ownership clear:

- `actions/cache` owns `target/`.
- `Swatinem/rust-cache` owns Cargo registry and git source caches.

Keeping `target/` out of `Swatinem/rust-cache` prevents overlapping cache
archives and makes the debug/release target cache keys explicit in the workflow
files.

## Debug and release separation

Debug and release builds use separate cache families because their compiler
outputs are not interchangeable.

Debug jobs run:

```powershell
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --locked
cargo build --locked
```

Release jobs run:

```powershell
cargo build --release --locked --target $env:TARGET
```

The cache path is the whole `target/` directory, but the key family keeps debug
and release cache entries separate.

## R2 and sccache

R2-backed `sccache` is intentionally not part of the current workflow state.

The workflows do not set:

- `RUSTC_WRAPPER`
- `SCCACHE_BUCKET`
- `SCCACHE_REGION`
- `SCCACHE_ENDPOINT`
- `SCCACHE_S3_KEY_PREFIX`
- `CARGO_INCREMENTAL=0` for sccache

There are also no required `R2_*` repository variables or secrets for CI.

If R2 `sccache` is reintroduced later, this document should be updated together
with the workflow changes. The important design question then is whether the
extra shared compiler-output layer is worth the operational cost and cache
maintenance complexity.

## When to change the key

Update the `hashFiles(...)` inputs when a file outside the current set affects
compiled output. Common examples:

- new shader directories
- generated Rust sources
- build-script inputs
- native resource files consumed by `build.rs`

Update the fixed key parts when changing:

- Rust version
- target triple
- debug/release build profile assumptions

Do not add branch names, PR numbers, workflow names, or tags to the cache key
unless the intent is to stop sharing cache entries across those runs.
