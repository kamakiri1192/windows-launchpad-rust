# CI cache strategy

The CI workflows use two cache layers:

1. GitHub Actions cache stores each workflow's Rust `target/` directory for an exact source snapshot.
2. Cloudflare R2-backed `sccache` is enabled whenever R2 is configured.

This keeps normal reruns fast with GitHub cache while still letting R2 collect compiler outputs from any compile work that remains. R2 is the shared layer that crosses PR checks, PR review binaries, and release builds.

## Why this exists

GitHub Actions cache is useful for Cargo registry and git source caches, but it is scoped by branch and pull request refs. That makes it hard for one pull request to warm the build cache for another pull request.

`sccache` keys compiler outputs by the actual compiler inputs instead of the pull request number, so the same dependency crate can be reused across PRs when the Rust version, target, and compiler flags match.

## Cache flow

1. Restore `target/` from GitHub Actions cache using a key derived from the Rust version, target triple, source files, shaders, and CI workflow.
2. Enable R2 `sccache` when the repository variables and secrets are configured.
3. If the GitHub key is an exact hit, Cargo should have little compiler work left, so R2 operations stay small.
4. Run formatting, Clippy, tests, and build.
5. On success, save the current `target/` directory back to GitHub Actions cache under the exact source key.

R2 uploads happen during Cargo commands, not in a separate upload step. When `RUSTC_WRAPPER=sccache` is active, `sccache` reads and writes compiler outputs while `cargo clippy`, `cargo test`, or `cargo build` invokes `rustc`.

The second run of the same source snapshot should hit the workflow's GitHub target cache. R2 still stays enabled, but it only reads or writes for compiler work that Cargo actually performs.

## R2 cache layout

The R2 `sccache` prefixes intentionally do not include a PR number, branch name, tag, or workflow name:

- `windows-launchpad-rust/x86_64-pc-windows-msvc/rust-1.89.0/debug`
- `windows-launchpad-rust/x86_64-pc-windows-msvc/rust-1.89.0/release`

This is what allows cache hits across PR checks, review binary builds, and release builds when the compiler inputs match. Debug and release builds use separate prefixes because their compiler flags differ.

## Required repository settings

Set these repository variables:

- `R2_ACCOUNT_ID`: Cloudflare account ID.
- `R2_SCCACHE_BUCKET`: R2 bucket name used by sccache.

Set these repository secrets:

- `R2_ACCESS_KEY_ID`: R2 API token access key ID.
- `R2_SECRET_ACCESS_KEY`: R2 API token secret access key.

The R2 API token needs object read/write access to the bucket.

## R2 sccache behavior

When enabled, the workflow configures `sccache` for Cloudflare R2:

- `SCCACHE_BUCKET=${R2_SCCACHE_BUCKET}`
- `SCCACHE_REGION=auto`
- `SCCACHE_ENDPOINT=https://${R2_ACCOUNT_ID}.r2.cloudflarestorage.com`
- `SCCACHE_S3_KEY_PREFIX=windows-launchpad-rust/x86_64-pc-windows-msvc/rust-1.89.0/<profile>/`
- `SCCACHE_S3_USE_SSL=true`
- `SCCACHE_IGNORE_SERVER_IO_ERROR=1`

`SCCACHE_IGNORE_SERVER_IO_ERROR=1` keeps CI builds working if R2 is temporarily unavailable.

`CARGO_INCREMENTAL=0` is set when R2 sccache is enabled so CI compiler calls are more cacheable.

The shared setup action smoke-tests R2 with `sccache rustc -vV` before enabling `RUSTC_WRAPPER` for the Cargo steps. If R2 credentials or bucket settings are wrong, the workflow logs a warning and continues without R2.

## R2 cost controls

Cloudflare R2's Standard storage free tier includes 10 GB-month of storage, 1 million Class A operations, 10 million Class B operations, and free egress. The target budget for this repository is lower: keep the combined `debug` and `release` `sccache` prefixes around 4 GiB.

The `.github/workflows/r2-sccache-maintenance.yml` workflow runs daily and can also be run manually. It lists both R2 cache prefixes and deletes the oldest objects until their combined size is below 4 GiB.

Also configure an R2 object lifecycle rule for the same prefix:

- Prefix: `windows-launchpad-rust/x86_64-pc-windows-msvc/rust-1.89.0/debug/`
- Prefix: `windows-launchpad-rust/x86_64-pc-windows-msvc/rust-1.89.0/release/`
- Action: expire objects after 7 days

That lifecycle rule is intentionally short because GitHub Actions cache should handle repeated runs of the same PR commit. R2 is only the fallback for cold or changed builds.

You can set the rule from the Cloudflare dashboard under the bucket's Object Lifecycle Rules, or with Wrangler:

```powershell
npx wrangler r2 bucket lifecycle add <bucket-name> launchpad-sccache-7d windows-launchpad-rust/x86_64-pc-windows-msvc/rust-1.89.0/debug/ --expire-days 7
npx wrangler r2 bucket lifecycle add <bucket-name> launchpad-sccache-release-7d windows-launchpad-rust/x86_64-pc-windows-msvc/rust-1.89.0/release/ --expire-days 7
```

## Checking the cache

After a run, check the `sccache --show-stats` output. Useful numbers are:

- cache hits
- cache misses
- compile requests
- non-cacheable calls

If the hit rate is low, check whether the Rust version, target, profile, or feature flags differ between the warm-up workflow and the PR workflow.
