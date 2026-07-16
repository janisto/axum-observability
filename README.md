# axum-observability

Focused Axum middleware for request IDs, W3C trace correlation,
request-scoped structured events, and one terminal access record per response.

## Why this package exists

Managed platforms such as Cloud Run already collect container output.
Applications should only need to write structured JSON to standard output
(`stdout`); the platform can handle ingestion and delivery.

Compared with sending logs through an in-process cloud logging client, this
reduces container CPU, memory, and network use by removing logging API calls,
authentication, buffering, batching, and retry work from the application. It
also avoids the dependency and maintenance cost of a cloud logging SDK,
including its credentials, configuration, and upgrades.

This crate turns that simple pipeline into useful production observability. It
provides validated request IDs, strict W3C trace correlation, request-scoped
fields, and one structured terminal access record. Application and access logs
can share the same correlation metadata, making records from a request easier
to find and understand.

Cloud presets map the same contract to provider-oriented fields without
coupling application code to a cloud SDK. The crate focuses on structured
logging and request correlation: it does not create spans for a tracing
backend, configure OpenTelemetry, export metrics, or ship logs.

## Package scope

`ObservabilityLayer` is one Tower layer for Axum 0.8. The application keeps
control of its `tracing` subscriber, writer, filter, panic recovery, listener,
and deployment policy. `JsonLayer` is composable and never installs global
state itself.

This is an independently maintained crate, not official Axum middleware.

## Requirements and installation

The minimum supported Rust version is 1.96.1. The first functional release is
v0.2.0 and targets Axum 0.8.9.

```toml
[dependencies]
axum = "0.8.9"
axum-observability = "0.2.0"
tracing = "0.1.44"
tracing-subscriber = { version = "0.3.23", features = ["env-filter"] }
```

While the crate is below 1.0, minor versions may evolve the public API. Patch
versions preserve documented behavior.

## GCP setup

When this documentation shows one configuration, it uses GCP. Complete
provider-neutral, GCP, AWS, Azure, and local-wrapper examples are available in
[`examples`](examples) and [EXAMPLES.md](EXAMPLES.md).

```rust
use axum::{Router, routing::get};
use axum_observability as obs;
use tracing_subscriber::prelude::*;

let config = obs::ObservabilityConfig::default().with_preset(obs::Preset::Gcp);

tracing_subscriber::registry()
    .with(
        tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
    )
    .with(config.json_layer(std::io::stdout))
    .init();

let app = Router::new()
    .route("/health", get(|| async { "ok" }))
    .layer(obs::ObservabilityLayer::new(config));
# let _: Router = app;
```

`JsonLayer` writes one complete JSON object per line. Keep
`axum_observability::request=info` enabled in `RUST_LOG` when application events
need correlation from the request span. Terminal access records carry their own
validated correlation fields, so a surviving WARN or ERROR access event stays
complete even when the INFO request span is filtered.

Handlers can extract the validated context directly:

```rust
use axum_observability::RequestContext;

async fn handler(context: RequestContext) {
    tracing::info!(item_count = 3_u64, "loaded items");
    assert!(!context.request_id().is_empty());
}
```

Event fields cannot overwrite package-owned request correlation values.

## Middleware placement

Observability must wrap response-producing middleware so it sees the final
status and body. Axum applies the last layer first, so add it last:

```rust
use std::time::Duration;

use axum::{Router, http::StatusCode};
use axum_observability::{ObservabilityConfig, ObservabilityLayer};
use tower_http::{
    catch_panic::CatchPanicLayer,
    cors::CorsLayer,
    limit::RequestBodyLimitLayer,
    timeout::TimeoutLayer,
};

let app = Router::new()
    // Add routes and application middleware first.
    .layer(RequestBodyLimitLayer::new(1024 * 1024))
    .layer(CorsLayer::new())
    .layer(TimeoutLayer::with_status_code(
        StatusCode::REQUEST_TIMEOUT,
        Duration::from_secs(30),
    ))
    .layer(CatchPanicLayer::new())
    .layer(ObservabilityLayer::new(ObservabilityConfig::default()));
# let _: Router = app;
```

Integration tests cover this order for recovered panics and timeouts. The crate
does not recover panics itself. If a panic must become a final 500 response,
install recovery middleware inside `ObservabilityLayer` as shown.

## Request and trace context

The default header is `X-Request-ID`. Exactly one incoming value is accepted
when it contains 1-128 ASCII URI-unreserved characters: `A-Z`, `a-z`, `0-9`,
`-`, `.`, `_`, and `~`. Missing, empty, duplicate, oversized, non-ASCII, or
otherwise invalid values are replaced. The fallback is 128 random bits encoded
as 32 lowercase hexadecimal characters.

The selected value is available from:

- the `RequestContext` extractor and request extension;
- `request_id()` and `correlation_id()`;
- application events inside an enabled package request span;
- the terminal access record; and
- the configured response header, unless disabled.

Configuration can change the request/response header name, disable the response
header, narrow validation, or supply a fallible generator. Generated values
still pass the package baseline. A generator is tried at most twice before the
package-owned fallback is used, and callback failure never produces an invalid
ID.

`traceparent` parsing rejects duplicates, uppercase hexadecimal, zero trace or
parent IDs, invalid framing and flags, and oversized input. Version `00` must
use its exact framing; well-formed future-version extensions are retained.
Repeated `tracestate` values are combined in wire order and accepted only when
their grammar, unique-key rule, 32-member limit, and 512-byte limit pass. An
invalid `tracestate` is discarded without invalidating a valid `traceparent`.

With valid trace context, `correlation_id` is the trace ID; otherwise it is the
request ID. The incoming parent ID belongs to the caller. The crate does not
claim it as a span created by this service, manufacture a current span ID, or
mutate outbound trace headers.

## Log contract

Every JSON event produced by `JsonLayer` contains `timestamp`, `target`, and
`level` (`severity` on GCP). GCP maps `TRACE` and `DEBUG` to `DEBUG`, and `WARN`
to Cloud Logging's canonical `WARNING`; `INFO` and `ERROR` are unchanged.
Events that record a message keep it under `message`. Typed `tracing` fields
remain JSON numbers, booleans, and strings.
Application errors recorded through `tracing::field::Visit::record_error` use
their display text.

During a request, events inside the enabled package span also contain
`request_id` and `correlation_id`. Valid W3C context adds `trace_id`,
`parent_id`, `trace_flags`, and `trace_sampled`.

The terminal record always has `message = "request completed"` and includes:

| Field | Meaning |
| --- | --- |
| `method` | HTTP method |
| `path` | Escaped concrete path without query string |
| `path_template` | Axum `MatchedPath`, when available |
| `operation_id` | Explicit `OperationId` request or response extension |
| `status` | Final response status when known |
| `duration_ms` | Non-negative handling and streaming time in milliseconds |
| `remote_ip` | `ConnectInfo<SocketAddr>` peer IP, when present |
| `user_agent` | One unambiguous raw User-Agent value |
| `terminal_reason` | `body_error`, `service_error`, or `response_dropped` on abnormal completion |
| `error` | Controlled package error description on body or service failure |

Normal completion omits `terminal_reason` and `error`. The default level is
`ERROR` for 5xx, `WARN` for 4xx, and `INFO` otherwise. Application events cannot
replace package correlation, envelope, or provider fields. Access enrichment
cannot replace terminal access fields either.

`path_template` is the low-cardinality aggregation key. Concrete `path` remains
useful for individual-request diagnostics and can have unbounded cardinality.

## Operation IDs

An outer Tower layer cannot inspect request extensions inserted after the
request has been consumed by route middleware. Route-specific operation IDs
should therefore use Axum's native response-extension path:

```rust
use axum::{Extension, http::StatusCode};
use axum_observability::OperationId;

async fn list_items() -> (Extension<OperationId>, StatusCode) {
    (Extension(OperationId::new("list-items")), StatusCode::OK)
}
```

An `OperationId` already present before the request reaches observability is
also supported. A response extension takes precedence because it is closest to
the selected route.

## Cloud presets

Select one preset on the shared `ObservabilityConfig`; `json_layer` and the
terminal middleware then use the same field convention.

- `Gcp` uses `severity`, `logging.googleapis.com/trace`,
  `logging.googleapis.com/trace_sampled`, and a structured `httpRequest` access
  field. The trace field is always the bare validated 32-character W3C trace
  ID. The crate never prepends `projects/{project}/traces/` and never emits a
  fake `logging.googleapis.com/spanId`.
- `Aws` adds `xray_trace_id` in `1-8hex-24hex` form. It does not create an X-Ray
  segment or parse `X-Amzn-Trace-Id`.
- `Azure` adds `operation_Id` and `operation_ParentId`. It does not initialize
  an Azure SDK or parse legacy `Request-Id` headers.
- `Default` emits provider-neutral fields using `level`.

Provider fields are derived only from a validated W3C trace ID. They correlate
logs; trace creation, sampling policy, and export remain application concerns.
Google Cloud's current [preferred trace field
format](https://docs.cloud.google.com/trace/docs/trace-log-integration) is the
bare trace ID.

## Response and failure behavior

The body wrapper owns a one-shot terminal guard:

- an already-ended body completes before its EOF state is exposed;
- a streaming body completes when it returns EOF;
- a body error emits one `body_error` record and passes the original body error
  to the consumer;
- an inner service error emits one `service_error` record and returns the
  original service error;
- dropping an unfinished response or service future emits one
  `response_dropped` record; and
- once the guard completes, later polling or drop cannot emit a duplicate.

Status and duration reflect the latest trustworthy state. If the response was
never produced, status is omitted. The monotonic clock is saturating, so a bad
custom clock cannot produce a negative duration.

Custom generator, validator, level-mapper, and access-enricher panics are
contained with safe fallback behavior. A finish-time clock failure falls back
to the request start; a custom clock must not panic when the request begins.
Formatter serialization and writer errors do not replace the HTTP response.
Writer failures can still mean a log record was not delivered; applications
remain responsible for choosing and monitoring the output destination.

## Configuration

`ObservabilityConfig` is builder-based:

| Method | Default | Purpose |
| --- | --- | --- |
| `with_preset` | `Preset::Default` | Select one provider field convention |
| `with_request_id_header` | `x-request-id` | Set the request and response correlation header |
| `with_response_header` | `true` | Enable or disable response-header injection |
| `with_request_id_generator` | random 128-bit ID | Supply a fallible generator, tried at most twice |
| `with_request_id_validator` | accepts baseline | Narrow accepted IDs without weakening the baseline |
| `with_status_level_mapper` | 5xx/4xx/other mapping | Map final status to a `tracing::Level` |
| `with_clock` | `Instant::now` | Supply a monotonic clock, primarily for deterministic tests |
| `with_access_enricher` | no extra fields | Add synchronous application-owned terminal fields |

Unknown options do not exist: configuration is compile-time checked. Invalid
HTTP header names return an error immediately. Enrichment values must be safe
to log; the crate does not redact application-owned fields.

## Proxy trust and privacy

`remote_ip` comes only from Axum `ConnectInfo<SocketAddr>`. The crate never
parses `Forwarded` or `X-Forwarded-For`, because trusting caller-controlled
forwarding headers without a known proxy boundary permits spoofing. Configure
trusted proxy handling before constructing `ConnectInfo` if the original client
address is required.

The terminal schema never logs query strings, request or response bodies,
cookies, authorization values, arbitrary headers, or forwarded-IP headers. The
GCP request URL uses the same query-free concrete path.

There is no automatic redaction of application `tracing` fields or access
enrichment. Applications remain responsible for keeping credentials, personal
data, and secrets out of those values.

## Troubleshooting

| Symptom | Cause | Correction |
| --- | --- | --- |
| No access record | Its selected level is filtered, or the writer failed | Enable the `axum_observability::access` level and verify the writer |
| WARN/ERROR access record lacks application span fields | INFO request span is filtered | Terminal correlation remains complete; enable `axum_observability::request=info` for correlated application events |
| Timeout or recovered panic has the wrong status | Observability is inside response-producing middleware | Add `ObservabilityLayer` last so it wraps recovery and timeout layers |
| `operation_id` is absent | A route middleware inserted it only on the consumed request | Return `Extension(OperationId)` on the response |
| `remote_ip` is absent | No `ConnectInfo<SocketAddr>` extension exists | Serve the router with connect-info support or insert a trusted peer extension |
| Caller request ID is replaced | It is missing, duplicate, invalid, or rejected by custom policy | Send one URI-unreserved value of at most 128 bytes |
| GCP trace link is absent | `traceparent` is missing or invalid | Send one valid lowercase W3C `traceparent`; do not provide a project-qualified value |
| Duplicate framework access lines | Another access logger remains enabled | Disable the competing access logger when this crate owns terminal records |

## Compatibility and development

The crate supports Rust 1.96.1 or newer and Axum 0.8.9. Beginning with 1.0.0,
exported APIs, configuration defaults, structured fields, and supported runtime
versions are compatibility contracts. Breaking changes require a new major
version, explicit changelog coverage, and migration guidance.

Development uses [just](https://github.com/casey/just). The normal gates are:

```bash
just qa
just package-check
```

`just qa` runs formatting, Clippy with warnings denied, tests, doctests,
dependency policy, and the RustSec audit. `just package-check` creates the exact
crate archive, verifies its allowlisted contents and size, compiles the packaged
crate, and runs an isolated consumer against it. Maintainers should follow the
public [release architecture and guide](RELEASE.md).

## Mutation and fuzz testing

The crate has explicit mutation and parser-fuzzing campaigns:

```bash
just mutation
just fuzz traceparent 30
```

Mutation testing runs outside `just qa`; see [MUTATION.md](MUTATION.md) for the
reviewed baseline and narrow exclusions. Add a behavioral test when a surviving
mutant exposes a real contract gap. Equivalent transformations do not need
artificial assertions.

Fuzz targets cover request IDs, `traceparent`, and `tracestate`. Fuzzing requires
a Rust nightly toolchain for libFuzzer sanitizer instrumentation; the crate's
build and MSRV remain on stable Rust.

## References

- [Axum middleware](https://docs.rs/axum/latest/axum/middleware/index.html)
  documents middleware placement and ordering.
- [`Extension`](https://docs.rs/axum/latest/axum/struct.Extension.html) and
  [`IntoResponseParts`](https://docs.rs/axum/latest/axum/response/trait.IntoResponseParts.html)
  document request and response extensions.
- [`http-body::Body`](https://docs.rs/http-body/latest/http_body/trait.Body.html)
  defines frame polling, EOF, and size-hint behavior.
- [`tracing-subscriber::Layer`](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/trait.Layer.html)
  and [`EnvFilter`](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html)
  define formatter composition and filtering.
- [W3C Trace Context](https://www.w3.org/TR/trace-context/) defines strict
  `traceparent` and `tracestate` syntax and the caller-owned parent ID.
- [Google Cloud trace and log integration](https://docs.cloud.google.com/trace/docs/trace-log-integration)
  documents the bare trace ID as the preferred trace field format.
- [Google Cloud structured logging](https://docs.cloud.google.com/logging/docs/structured-logging)
  documents `severity`, `message`, `httpRequest`, and special trace fields.
- [AWS X-Ray trace IDs](https://docs.aws.amazon.com/xray/latest/devguide/xray-api-sendingdata.html)
  documents conversion from W3C to `1-8hex-24hex` form.
- [Azure Application Insights data model](https://learn.microsoft.com/en-us/azure/azure-monitor/app/data-model-complete)
  documents operation correlation fields.

## License

[MIT](LICENSE)
