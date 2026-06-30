# Shared S3 sccache for CI

This repository can use an S3-backed `sccache` store so Rust builds can reuse compiler outputs across pull requests.

## Why this exists

GitHub Actions cache is useful for Cargo registry and git source caches, but it is scoped by branch and pull request refs. That makes it hard for one pull request to warm the build cache for another pull request.

`sccache` keys compiler outputs by the actual compiler inputs instead of the pull request number, so the same dependency crate can be reused across PRs when the Rust version, target, and compiler flags match.

## Cache layout

The workflows use stable prefixes that intentionally do not include a PR number or branch name:

- `windows-launchpad-rust/x86_64-pc-windows-msvc/rust-1.89.0/debug`
- `windows-launchpad-rust/x86_64-pc-windows-msvc/rust-1.89.0/release`

This is what allows cache hits across different PRs.

## Required repository variables

Set these repository variables before enabling the cache:

- `SCCACHE_BUCKET`: S3 bucket name used by sccache.
- `AWS_REGION`: S3 bucket region.
- `AWS_SCCACHE_ROLE_ARN`: IAM role ARN used by trusted cache warm-up workflows.

## PR behavior

Pull request workflows use the shared S3 cache in read-only mode and do not receive AWS write credentials. This avoids giving untrusted PR code write access to the cache bucket.

The PR workflow still falls back to a normal local build if the S3 cache is not configured or unavailable.

## Warming the cache

The trusted warm-up workflow runs on `main` and can write to S3. This keeps the shared cache warm for later PRs.

After a run, check the `sccache --show-stats` output. Useful numbers are:

- cache hits
- cache misses
- compile requests
- non-cacheable calls

If the hit rate is low, check whether the Rust version, target, profile, or feature flags differ between the warm-up workflow and the PR workflow.
