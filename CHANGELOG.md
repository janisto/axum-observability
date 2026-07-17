# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project uses
[Semantic Versioning](https://semver.org/).

## [Unreleased]

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
