# Repository instructions

## Documentation

- `README.md` is primarily for human users and contributors. Keep installation,
  usage, public API, and operational guidance there.
- Put instructions needed specifically by coding agents in `AGENTS.md`. When
  agent-specific guidance changes, update this file rather than adding it to
  `README.md`.
- Keep `plans/` ignored. Planning status is local and must not ship in the crate
  or repository.

## Engineering changes

- Inspect the relevant implementation, callers, and existing tests before
  editing.
- Prefer the smallest safe change that solves the problem.
- Reuse existing patterns and utilities, refactoring them when needed, instead
  of creating parallel abstractions or adding dependencies.
- Preserve `#![forbid(unsafe_code)]` and use stable Rust APIs.
- Do not add OpenTelemetry, a cloud SDK, a global subscriber, or logging of
  queries, bodies, credentials, cookies, arbitrary headers, or forwarded IPs.

## Public API and documentation

- Update applicable tests, README content, examples, rustdoc, and changelog
  entries when public behavior changes.
- Keep `CHANGELOG.md` in Keep a Changelog format with an `Unreleased` section,
  ISO-dated bracketed versions, applicable change categories, and comparison
  links.
- Keep examples minimal, runnable, and aligned with the documented API.
- Treat exported APIs, structured log fields, defaults, and supported runtime
  versions as compatibility contracts.
- Document breaking changes explicitly and provide migration guidance when
  applicable.

## Pull requests

- Use `<type>[optional scope]: <description>` for the title. Prefer no scope;
  include one only when it materially improves clarity.
- Example: `fix: preserve terminal correlation`.
- Use `feat`, `fix`, `docs`, `test`, `refactor`, `perf`, `build`, `ci`, `chore`,
  or `revert` as the type.
- Keep each pull request focused. In the body, explain why the change is needed,
  what changed, how it was validated, and any remaining risk.
- Before opening a pull request, add applicable user-visible changes under
  `CHANGELOG.md` -> `[Unreleased]`. Skip entries for changes without meaningful
  user impact.
- Keep the title suitable for the final squash or merge commit.

## Commits

- Follow [Conventional Commits 1.0.0](https://www.conventionalcommits.org/en/v1.0.0/).
- Prefer no scope; include one only when it materially improves clarity. Write a
  short, imperative description.
- Example: `fix: preserve request ID`.
- Mark breaking changes with `!` and explain them in a `BREAKING CHANGE:` footer.
- Before committing, run `just qa` and `git diff --check`.
- Run `just package-check` when package contents or release metadata change.

## Tests

- Use the repository's `$adversarial-testing` skill when creating, updating, or
  reviewing tests.
- Test observable behavior, parser boundaries, failure recovery, cancellation,
  one-shot terminal effects, and forbidden sensitive output. Do not optimize
  for coverage numbers or mock interactions alone.
- Run `just mutation` when changing production logic or its focused tests. Add
  tests for meaningful surviving mutants, not equivalent transformations.
- Promote minimized property-test failures to named deterministic regression
  tests before fixing the production behavior.
- Never commit generated coverage, mutation, or package artifacts.

## Releases

- Prepare releases through a pull request titled `chore: prepare vX.Y.Z`.
- Update `Cargo.toml`, `Cargo.lock`, `CHANGELOG.md`, rustdoc, examples, and public
  documentation together when applicable.
- Run `just qa`, `just package-check`, `cargo publish --dry-run --locked`,
  `actionlint .github/workflows/ci.yml .github/workflows/release.yml`, and
  `git diff --check`.
- Merge a green pull request to `main`, then release the exact reviewed commit
  with an annotated tag `vX.Y.Z`.
- When drafting a stable GitHub Release, use **Generate release notes** and mark
  it as **Latest**. Edit the generated notes for accuracy and alignment with
  `CHANGELOG.md` before publishing.
- Never move an existing release tag or reuse a published crates.io version.
- Verify the GitHub Release, crates.io metadata and archive, docs.rs, and a fresh
  registry-backed consumer after publishing.
- Follow `RELEASE.md` for trusted publishing and recovery procedures.
