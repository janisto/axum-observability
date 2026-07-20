# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

The changes in this section target `2.0.0` and must not be published on the
`1.x` release line.

### Migration from 1.x

- Parse `trace_flags` as exactly two lowercase hexadecimal characters instead
  of a JSON number. This preserves leading zeroes and the wire representation.
- Replace queries that depend on synthetic body/service `error` summaries with
  `terminal_reason`, status, and severity. Rich error text remains omitted by
  default to avoid disclosing application details.
- Treat custom request-ID validators as caller-input narrowing only. Generated
  IDs always satisfy the crate baseline grammar and are not passed to the
  custom validator.
- Allow a fallible custom generator to run up to two times before the crate
  falls back. Generators with external side effects must therefore make those
  effects idempotent or avoid them.
- Replace `JsonLayer::new(writer, convention)` with
  `ObservabilityConfig::json_layer(writer)`. Version 2 has no direct-constructor
  compatibility shim, so middleware and formatter settings come from one
  configuration value.

### Added

- Added the specification-defined GCP `0.1.0` profile version, newest-supported
  resolution for `FieldConvention::Gcp`, exact typed pinning, and effective
  version introspection without network lookup.
- Added typed W3C Trace Context Level 1/Level 2 configuration, with Level 1 as
  the default and Level 2 random trace-ID flag projection.

### Changed

- Removed the v1 direct JSON-layer constructor so v2 exposes one coherent
  configuration path.
- Set crate and lock metadata to `2.0.0` so Cargo validation cannot package the
  breaking v2 surface under the v1 version.

- Aligned the GCP health example and integration fixture with privacy-safe
  request metadata, the shared `1.0.0` fixture value, stable `health_check`
  operation identity, and deterministic DEBUG/INFO output.
- Aligned correlation output with string `trace_flags`, selected-level
  `tracestate` grammar including empty members, conditional `trace_id_random`,
  and integer serialization for exact whole-millisecond durations.
- Classified `response_dropped` as an abnormal `ERROR` outcome regardless of
  committed status or the normal-response status mapper.
- Validate Axum `MatchedPath` parameter and terminal catch-all names before
  emitting the already-canonical route template; unsafe forms are omitted.
- Stop synthesizing fixed `error` summaries for body and service failures, and
  prevent application events from occupying access-catalog field names.

### Fixed

- Omit malformed percent-escaped raw paths instead of emitting them.
- Preserve sampling while omitting the Level 2 random flag for unknown future
  `traceparent` versions.

## [1.0.0] - 2026-07-17

### Changed

- Stabilized the public API, configuration defaults, structured log fields,
  Rust 1.97.0 support line, and Axum 0.8 support line as Semantic Versioning
  compatibility contracts. This release does not change runtime behavior from
  0.3.0.

## [0.3.0] - 2026-07-17

### Fixed

- Reported redacted formatter failures without unwinding through request
  handling or retrying failed event writes.
- Canonicalized the configured request-ID header before downstream handling,
  used one validated `RequestId` across context, spans, logs, and responses,
  and accepted opaque W3C future-version `traceparent` extensions.
- Classified terminal body and service failures at `ERROR`, preserved mapped
  levels for completed and abandoned responses, and contained initial clock
  panics without replacing request handling.

### Changed

- Replaced the pre-release API and log contracts with validated typed headers,
  static operation IDs, `FieldConvention`, a nameable service and extractor
  rejection, and privacy-preserving opt-in request metadata.
- Relaxed dependency requirements across the supported Axum 0.8 line, reduced
  the default feature graph, and made peer-IP support an explicit `peer-ip`
  feature.
- Raised the minimum supported Rust version from 1.96.1 to 1.97.0 and aligned
  local, CI, release, example, and issue-reporting guidance.

## [0.2.0] - 2026-07-16

### Added

- Request ID validation, generation, request extensions, and response headers.
- Strict inbound W3C trace-context correlation.
- One terminal access record covering response completion, already-ended
  bodies, streaming errors, service errors, and early drop.
- Composable JSON logging with Default, Google Cloud, AWS, and Azure presets.
- Canonical Google Cloud severity names, including `WARNING` for Rust `WARN`.
- Filter-independent correlation fields on terminal records and exact bare GCP
  trace IDs from validated W3C context.
- Route operation IDs through Axum response extensions, with pre-seeded request
  extensions retained as a fallback.
- Runnable examples, package validation, CI, mutation testing, and fuzz targets.

## [0.1.0] - 2026-07-16

### Added

- Minimal dependency-free crate used to establish crates.io ownership and
  configure Trusted Publishing. This version intentionally exposed no
  middleware API.

[Unreleased]: https://github.com/janisto/axum-observability/compare/v1.0.0...HEAD
[1.0.0]: https://github.com/janisto/axum-observability/compare/v0.3.0...v1.0.0
[0.3.0]: https://github.com/janisto/axum-observability/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/janisto/axum-observability/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/janisto/axum-observability/releases/tag/v0.1.0
