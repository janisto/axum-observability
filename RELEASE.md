# Release guide

This is a maintainer procedure. v0.1.0 was the one-time manual crates.io
bootstrap. Every later version is published by the reviewed GitHub Release
workflow using crates.io Trusted Publishing; crates.io rejects token-based
fallback publication.

## Prepare

1. Update `Cargo.toml`, `Cargo.lock`, rustdoc, README, examples, and
   `CHANGELOG.md` to the same version and behavior.
2. From the exact clean reviewed commit, run:

   ```bash
   just qa
   just package-check
   cargo publish --dry-run --locked
   cargo package --locked --list
   git diff --check
   git status --short
   ```

3. Inspect the package file list and archive. Never use `--allow-dirty`.
4. Verify the release commit is on `main`, then create an annotated SemVer tag
   at that exact commit.

## Publish through OIDC

Push the annotated tag, then publish a non-prerelease GitHub Release from it.
`.github/workflows/release.yml` checks out that tag and verifies its annotation,
SemVer shape, exact commit, `main` ancestry, and Cargo package version. It runs
formatting, Clippy, tests, rustdoc, package verification, and a publish dry-run
before requesting a short-lived crates.io token through OIDC.

Verify the registry metadata, checksum, README, license, dependency list,
docs.rs build, and a fresh external consumer before publishing the matching
release completion notice.

After functional v0.2.0 is verified, migrate `axum-playground` against exact
registry version `=0.2.0`. Do not use a path dependency as a pre-publication
proof.
