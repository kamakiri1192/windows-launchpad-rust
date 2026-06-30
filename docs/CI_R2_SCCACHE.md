# GitHub target cache with R2 sccache fallback

The PR CI workflow uses two cache layers:

1. GitHub Actions cache stores the Rust `target/` directory for an exact source snapshot.
2. Cloudflare R2-backed `sccache` is enabled only when the exact GitHub `target/` cache is not found.

This keeps normal reruns on GitHub cache and uses R2 only for cold or changed PR builds.

## Why this exists

GitHub Actions cache is useful for Cargo registry and git source caches, but it is scoped by branch and pull request refs. That makes it hard for one pull request to warm the build cache for another pull request.

`sccache` keys compiler outputs by the actual compiler inputs instead of the pull request number, so the same dependency crate can be reused across PRs when the Rust version, target, and compiler flags match.

## Cache flow

1. Restore `target/` from GitHub Actions cache using a key derived from the Rust version, target triple, source files, shaders, and CI workflow.
2. If that key is an exact hit, build normally without enabling R2.
3. If the key is a miss or only a restore-key match, enable R2 `sccache`.
4. Run formatting, Clippy, tests, and build.
5. On success, save the current `target/` directory back to GitHub Actions cache under the exact source key.

The second run of the same PR commit should hit the GitHub target cache and avoid R2 operations.

## R2 cache layout

The R2 `sccache` prefix intentionally does not include a PR number or branch name:

- `windows-launchpad-rust/x86_64-pc-windows-msvc/rust-1.89.0/debug`

This is what allows cache hits across different PRs.

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
- `SCCACHE_S3_KEY_PREFIX=windows-launchpad-rust/x86_64-pc-windows-msvc/rust-1.89.0/debug/`
- `SCCACHE_S3_USE_SSL=true`
- `SCCACHE_IGNORE_SERVER_IO_ERROR=1`

`SCCACHE_IGNORE_SERVER_IO_ERROR=1` keeps CI builds working if R2 is temporarily unavailable.

## R2 cost controls

Cloudflare R2's Standard storage free tier includes 10 GB-month of storage, 1 million Class A operations, 10 million Class B operations, and free egress. The target budget for this repository is lower: keep the `sccache` prefix around 4 GiB.

The `.github/workflows/r2-sccache-maintenance.yml` workflow runs daily and can also be run manually. It lists the R2 cache prefix and deletes the oldest objects until the prefix is below 4 GiB.

Also configure an R2 object lifecycle rule for the same prefix:

- Prefix: `windows-launchpad-rust/x86_64-pc-windows-msvc/rust-1.89.0/debug/`
- Action: expire objects after 7 days

That lifecycle rule is intentionally short because GitHub Actions cache should handle repeated runs of the same PR commit. R2 is only the fallback for cold or changed builds.

You can set the rule from the Cloudflare dashboard under the bucket's Object Lifecycle Rules, or with Wrangler:

```powershell
npx wrangler r2 bucket lifecycle add <bucket-name> launchpad-sccache-7d windows-launchpad-rust/x86_64-pc-windows-msvc/rust-1.89.0/debug/ --expire-days 7
```

## Checking the cache

After a run, check the `sccache --show-stats` output. Useful numbers are:

- cache hits
- cache misses
- compile requests
- non-cacheable calls

If the hit rate is low, check whether the Rust version, target, profile, or feature flags differ between the warm-up workflow and the PR workflow.
