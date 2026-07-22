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
- Custom request-ID validators apply only to caller input and may broaden it
  within Axum's native text-header boundary. Generated IDs satisfy the crate
  baseline grammar and are not passed to the custom validator. Replace a v1
  `Fn(&RequestId) -> bool` callback with a v2 `Fn(&str) -> bool` callback; the
  typed extractor represents the value selected by that configured policy.
- Read accepted `TraceContext::traceparent()` values as optional UTF-8 and use
  `traceparent_bytes()` when an opaque future-version suffix can contain raw
  HTTP `obs-text`.
- A fallible custom generator is invoked once before the crate-owned fallback,
  so one replacement request cannot duplicate generator side effects.
- Replace `JsonLayer::new(writer, convention)` with
  `ObservabilityConfig::json_layer(writer)`. Version 2 has no direct-constructor
  compatibility shim. Finalize one configuration before constructing the JSON
  layer and middleware; the layer snapshots its convention and is not updated
  by later builder calls.

### Added

- Added GCP, AWS, and Azure provider field conventions.
- Added typed W3C Trace Context Level 1/Level 2 configuration, with Level 1 as
  the default and Level 2 random trace-ID flag projection.

### Changed

- Removed the v1 direct JSON-layer constructor so v2 exposes one config-owned
  JSON-layer construction path; callers must finalize and reuse the same
  unchanged configuration for middleware coherence.
- Set crate and lock metadata to `2.0.0` so Cargo validation cannot package the
  breaking v2 surface under the v1 version.
- Documented LF-terminated NDJSON as the writer boundary and strengthened raw
  framing coverage for independently parseable records.

- Aligned the GCP health example and integration fixture with privacy-safe
  request metadata, the shared `1.0.0` fixture value, stable `health_check`
  operation identity, and deterministic DEBUG/INFO output.
- Aligned correlation output with string `trace_flags`, selected-level
  `tracestate` grammar including empty members, conditional `trace_id_random`,
  and integer serialization for exact whole-millisecond durations.
- Classified `response_dropped` as an abnormal `ERROR` outcome regardless of
  committed status or the normal-response status mapper.
- Trust nonempty Axum `MatchedPath` as authoritative framework route metadata,
  including literal-star static routes.
- Stop synthesizing fixed `error` summaries for body and service failures.
  Authenticate package access payloads by their internal callsite and preserve
  ordinary application fields outside the exact package-owned set.

### Fixed

- Serialize each complete formatter record across partial writer calls so
  concurrent events cannot interleave, and authenticate request-span metadata
  by the package callsite before accepting correlation fields.
- Protect exact package-owned fields contextually: application events can use
  access-only fields, exact aliases owned only by an inactive provider profile,
  and other non-owned provider-looking fields, while access enrichment cannot
  replace exact terminal fields. Speculative namespace prefixes are no longer
  reserved.
- Emit GCP `httpRequest.latency` with canonical ProtoJSON fractional widths:
  0, 3, 6, or 9 digits according to the required precision.
- Apply the RFC 9110 field-content boundary before custom request-ID validation,
  admitting internal space, tab, or a comma in one field-line; direct synthetic
  edge-whitespace values remain a native safety check after real HTTP parsing.
- Preserve framework-valid route metadata, HTTP-safe opaque future
  `traceparent` suffixes—including raw `obs-text`—without an invented length
  cap, valid `tracestate` beyond 512 characters, HTAB User-Agent values,
  custom-admitted request IDs, and nonempty static operation IDs.
- Preserve portable duration across conventions and omit only an
  unrepresentable GCP latency projection.

- Preserve every nonempty query-free path component exposed by `http::Uri`,
  including malformed percent forms and the asterisk form, without applying a
  second adapter grammar.
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
