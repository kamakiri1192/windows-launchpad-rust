# Shared S3 sccache for PR CI

The PR CI workflow can read from an S3-backed `sccache` store so Rust builds can reuse compiler outputs across pull requests.

This PR only adds the read path. It does not add a workflow that writes or warms the cache.

## Why this exists

GitHub Actions cache is useful for Cargo registry and git source caches, but it is scoped by branch and pull request refs. That makes it hard for one pull request to warm the build cache for another pull request.

`sccache` keys compiler outputs by the actual compiler inputs instead of the pull request number, so the same dependency crate can be reused across PRs when the Rust version, target, and compiler flags match.

## PR cache layout

The PR workflow uses a stable prefix that intentionally does not include a PR number or branch name:

- `windows-launchpad-rust/x86_64-pc-windows-msvc/rust-1.89.0/debug`

This is what allows cache hits across different PRs.

## Required repository variables for PR reads

Set these repository variables before enabling the PR cache read path:

- `SCCACHE_BUCKET`: S3 bucket name used by sccache.
- `AWS_REGION`: S3 bucket region.

## PR behavior

Pull request workflows use the shared S3 cache in anonymous read-only mode:

- `SCCACHE_S3_RW_MODE=READ_ONLY`
- `SCCACHE_S3_NO_CREDENTIALS=true`
- `SCCACHE_IGNORE_SERVER_IO_ERROR=1`

This avoids giving PR builds AWS credentials, including write credentials. It also means the configured bucket or prefix must allow unauthenticated reads for cache hits to work.

The PR workflow still falls back to a normal local build if the repository variables are missing, the bucket is private, or S3 is unavailable.

## Warming the cache

Cache objects must be written by a trusted workflow or another trusted process outside this PR. That writer can run on `main`, a scheduled workflow, or a manual workflow with AWS credentials.

If a future trusted GitHub Actions writer is added, keep it separate from `pull_request` and use an IAM role such as `AWS_SCCACHE_ROLE_ARN` there. Do not expose write credentials to PR code.

After a run, check the `sccache --show-stats` output. Useful numbers are:

- cache hits
- cache misses
- compile requests
- non-cacheable calls

If the hit rate is low, check whether the Rust version, target, profile, or feature flags differ between the warm-up workflow and the PR workflow.
