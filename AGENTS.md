# AGENTS.md

Instructions for coding agents working in this repository. Keep `README.md`
human-facing; implementation workflow and validation policy belong here.

## Engineering priorities

- Inspect the relevant implementation, callers, and tests before changing
  behavior. Prefer the smallest safe change.
- State the failure mode before architectural, security, or production-impacting
  changes.
- Preserve `#![forbid(unsafe_code)]`. Use safe body projection and stable
  `tracing-subscriber` APIs.
- Do not add OpenTelemetry, a cloud SDK, a global subscriber, or logging of
  queries, bodies, credentials, cookies, or arbitrary headers.
- Keep the public layer type stable. Avoid public traits and generic callback
  types unless a measured need justifies them.
- Keep `plans/` ignored. Planning status is local and must not ship in the crate
  or repository.

## Behavioral invariants

- Request IDs are 1–128 ASCII URI-unreserved characters. Custom policy may
  narrow but cannot weaken that baseline; failure always falls back to a safe
  package-generated ID.
- Only one valid `traceparent` is trusted. Invalid `tracestate` is discarded
  without invalidating a valid parent.
- The response body guard emits exactly one terminal record on EOF, error, or
  early drop. Response headers are not completion.
- Package request-span fields override colliding event fields. Provider fields
  are derived only from validated context.
- Access paths never contain the query string. Remote IP comes only from Axum
  `ConnectInfo`, never forwarded headers.

## Tests

Use `.agents/skills/adversarial-testing/SKILL.md` whenever tests are planned,
created, reviewed, or debugged. Prioritize parser boundaries, duplicate input,
off-by-one status mapping, streaming completion, cancellation/drop, reserved
field collisions, and forbidden sensitive output. Assert parsed JSON types and
nested shapes rather than substrings alone.

Run focused tests first, then the repository gate:

```bash
just fmt-check
just lint
just test
just test-doc
just qa
just package-check
```

Mutation and fuzzing are explicit campaigns and are not part of `just qa`.
Never commit `mutants.out/`, generated fuzz corpora, coverage output, or package
artifacts. `just fuzz` requires an installed Rust nightly toolchain because
libFuzzer sanitizer instrumentation uses unstable compiler flags.

## Dependencies and releases

- Rust 1.96.1, edition 2024, and Axum 0.8.9 are the v0.1 baseline.
- Keep `Cargo.lock` checked in. Change it only through Cargo.
- Every runtime dependency must serve a concrete crate behavior. Prefer the
  standard library and existing dependencies.
- Before release, run `just qa`, `just package-check`, `just audit`, and
  `cargo publish --dry-run --locked` from the exact reviewed commit.
- v0.2.0 is the first functional release and is published before any
  `axum-playground` integration. The playground must consume exact version
  `=0.2.0`; do not use a path dependency first.
