# AGENTS.md

Instructions for coding agents working in this repository.

`README.md` is for human users and contributors: setup, capabilities,
architecture, operations, and contribution entry points. `AGENTS.md` is for
coding agents: execution rules, implementation constraints, and validation
policy. Do not duplicate agent instructions into the README or turn this file
into human onboarding documentation.

## Engineering priorities

- Correctness first, then readability and maintainability, then performance.
- Inspect the relevant implementation, callers, and existing tests before
  changing behavior.
- Prefer the smallest safe change that solves the problem.
- Reuse existing local patterns and utilities, refactoring them when needed,
  instead of creating parallel abstractions or adding dependencies.
- State the failure mode before architectural, security, persistence, or
  production-impacting changes.
- Do not declare completion until implementation, validation, and remaining
  risks are reported.
- Keep source comments and documentation concise. Do not add progress
  narration, generated banners, emojis, or speculative TODOs.

## Pull requests

- Format titles as `type[optional scope]: description`. Prefer no scope;
  include one only when it materially improves clarity.
- Use `feat`, `fix`, `docs`, `test`, `refactor`, `perf`, `build`, `ci`, `chore`,
  or `revert` as the type. Example: `feat: add response size field`.
- Keep each pull request focused. In the body, explain why the change is
  needed, what changed, how it was validated, and any remaining risk.
- Keep the title suitable for the final squash or merge commit.
- Add applicable user-visible changes under `CHANGELOG.md` -> `[Unreleased]`.
  Skip entries for changes without meaningful user impact.

## Commits

- Follow [Conventional Commits 1.0.0](https://www.conventionalcommits.org/en/v1.0.0/).
- Prefer no scope; include one only when it materially improves clarity. Write
  a short, imperative description. Example: `fix: preserve request ID`.
- Mark breaking changes with `!` and explain them in a `BREAKING CHANGE:`
  footer.
- Before committing, run `just qa` and `git diff --check`.

## Repository constraints

- Keep `plans/` ignored. Planning and mutation-campaign notes are local and
  must not ship in the crate or repository.
- Keep compatible dependency requirements in `Cargo.toml`, exact repository
  resolution in `Cargo.lock`, and Dependabot in `lockfile-only` mode. Never
  edit the lockfile manually.
- Keep Cargo CLI tools shared by local and hosted checks aligned between the
  `Justfile` and workflows. Install them with `cargo install --locked` and do
  not hard-code tool versions that Dependabot cannot maintain; CI-only tools
  may remain workflow-only.
- Preserve `#![forbid(unsafe_code)]`, the Rust 1.97.0 support line, and stable
  Rust APIs.
- Do not add OpenTelemetry, a cloud SDK, a global subscriber, or logging of
  queries, bodies, credentials, cookies, arbitrary headers, or forwarded IPs.
- Treat exported APIs, structured log fields, defaults, and supported runtime
  versions as compatibility contracts.

## Public API and documentation

- Update applicable tests, README content, examples, rustdoc, and changelog
  entries when public behavior changes.
- Keep `CHANGELOG.md` in Keep a Changelog format with an `Unreleased` section,
  ISO-dated bracketed versions, applicable change categories, and comparison
  links.
- Keep examples minimal, runnable, and aligned with the documented API.
- Document breaking changes explicitly and provide migration guidance.

## Tests

- Use the repository's `$adversarial-testing` skill when creating, updating, or
  reviewing tests.
- Test observable behavior, parser boundaries, failure recovery, cancellation,
  one-shot terminal effects, and forbidden sensitive output. Do not optimize
  for coverage numbers or mock interactions alone.
- Run `just mutation` when changing production logic or its focused tests. Add
  tests for meaningful surviving mutants, not equivalent transformations.
- Promote minimized property-test failures to named deterministic regression
  tests before fixing production behavior.
- Never commit generated coverage, mutation, or package artifacts.

## Workflow security

- Use full release tags for third-party GitHub Actions, for example
  `actions/checkout@v7.0.0`. Do not use commit SHAs, moving branches, or major
  version tags.
- `just qa` must run `actionlint` and `zizmor --offline .` in addition to the
  repository's language checks.
- Do not add standalone repository scripts, including under `.github`. Enforce
  repository policy through the existing native test suite and tooling.
- Keep `.github/zizmor.yml` aligned with the exact-tag policy and the
  one-day Dependabot cooldown.

## Releases

- Prepare releases from a same-repository source branch named
  `release/prepare-vX.Y.Z` through a pull request titled
  `chore: prepare vX.Y.Z` that targets `main`.
- Use the `release/` namespace only for release preparation branches.
- The conditional `Consumer image build` job on a release preparation pull
  request is a build-only packaging and integration diagnostic. It does not run
  the image, validate emitted logs, or approve a release.
- For local image-build diagnosis, run
  `just e2e-image observability-e2e-local:manual`. The Justfile prefers Podman
  and falls back to Docker.
- Keep `e2e/README.md` self-contained as the public consumer-image interface.
  Independent audits are optional and informational; they never approve or
  block publication.
- Update `Cargo.toml`, `Cargo.lock`, `CHANGELOG.md`, rustdoc, examples, and
  public documentation together when applicable.
- Run `just qa`, `cargo publish --dry-run --locked`, and `git diff --check`.
- Merge a green pull request to `main`, create the annotated `vX.Y.Z` tag for
  the exact reviewed `main` commit, then publish the reviewed GitHub Release as
  the manual package-publication authorization; no external approval is
  required.
- When drafting a stable GitHub Release, use **Generate release notes** and mark
  it as **Latest**. Edit the notes for accuracy and alignment with
  `CHANGELOG.md` before publishing.
- Never move an existing release tag or reuse a published crates.io version.
- Verify the GitHub Release, crates.io metadata and archive, docs.rs, and a
  fresh registry-backed consumer after publishing.
- Follow `RELEASE.md` for trusted publishing and recovery procedures.
