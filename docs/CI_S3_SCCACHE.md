# Shared S3 sccache for CI

The PR CI workflow can use an S3-backed `sccache` store so Rust builds can reuse compiler outputs across pull requests.

When `AWS_SCCACHE_ROLE_ARN` is configured, PR CI authenticates to AWS with GitHub OIDC and uses the cache in read/write mode. Without that role, PR CI falls back to anonymous read-only mode.

## Why this exists

GitHub Actions cache is useful for Cargo registry and git source caches, but it is scoped by branch and pull request refs. That makes it hard for one pull request to warm the build cache for another pull request.

`sccache` keys compiler outputs by the actual compiler inputs instead of the pull request number, so the same dependency crate can be reused across PRs when the Rust version, target, and compiler flags match.

## Cache layout

The PR workflow uses a stable prefix that intentionally does not include a PR number or branch name:

- `windows-launchpad-rust/x86_64-pc-windows-msvc/rust-1.89.0/debug`

This is what allows cache hits across different PRs.

## Required repository variables

Set these repository variables before enabling S3 sccache:

- `SCCACHE_BUCKET`: S3 bucket name used by sccache.
- `AWS_REGION`: S3 bucket region.
- `AWS_SCCACHE_ROLE_ARN`: IAM role ARN that PR CI assumes with GitHub OIDC. If this is omitted, PR CI uses anonymous read-only S3 access instead.

## PR behavior

Pull request workflows have two cache modes.

With `AWS_SCCACHE_ROLE_ARN`, PR CI requests short-lived AWS credentials and uses the S3 cache in read/write mode. This means PR builds can populate the shared cache directly.

Without `AWS_SCCACHE_ROLE_ARN`, PR CI uses anonymous read-only mode:

- `SCCACHE_S3_RW_MODE=READ_ONLY`
- `SCCACHE_S3_NO_CREDENTIALS=true`
- `SCCACHE_IGNORE_SERVER_IO_ERROR=1`

Anonymous read-only mode requires the configured bucket or prefix to allow unauthenticated reads for cache hits to work.

The PR workflow skips S3 sccache entirely if `SCCACHE_BUCKET` or `AWS_REGION` is missing.

## AWS permissions

The IAM role used by `AWS_SCCACHE_ROLE_ARN` should be scoped to this repository's GitHub OIDC subject and to this cache prefix. It needs permission to read and write cache objects under:

- `s3://<SCCACHE_BUCKET>/windows-launchpad-rust/x86_64-pc-windows-msvc/rust-1.89.0/debug/*`

Because PR code can run build scripts, read/write mode should only be used when PRs are trusted for this repository.

The role policy needs object access for the cache prefix:

- `s3:GetObject`
- `s3:PutObject`

Add `s3:ListBucket` with a prefix condition for `windows-launchpad-rust/x86_64-pc-windows-msvc/rust-1.89.0/debug/*` only if the chosen bucket policy requires listing.

## Checking the cache

After a run, check the `sccache --show-stats` output. Useful numbers are:

- cache hits
- cache misses
- compile requests
- non-cacheable calls

If the hit rate is low, check whether the Rust version, target, profile, or feature flags differ between the warm-up workflow and the PR workflow.
