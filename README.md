# axum-observability

[![Crates.io](https://img.shields.io/crates/v/axum-observability.svg)](https://crates.io/crates/axum-observability)
[![Documentation](https://docs.rs/axum-observability/badge.svg)](https://docs.rs/axum-observability)
[![Rust version](https://img.shields.io/crates/msrv/axum-observability.svg)](#requirements-and-installation)
[![CI](https://img.shields.io/github/actions/workflow/status/janisto/axum-observability/ci.yml?branch=main&label=CI)](https://github.com/janisto/axum-observability/actions/workflows/ci.yml)
[![Socket Badge](https://badge.socket.dev/cargo/package/axum-observability)](https://socket.dev/cargo/package/axum-observability)

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

Field conventions map the same contract to provider-oriented fields without
coupling application code to a cloud SDK. The crate focuses on structured
logging and request correlation: it does not create spans for a tracing
backend, configure OpenTelemetry, export metrics, or ship logs.

## Why newline-delimited JSON

`JsonLayer` emits newline-delimited JSON (NDJSON, also called JSON Lines): each
application or access event is one compact, self-contained JSON object followed
by one LF (`\n`). The output is a stream of objects, never a JSON array.

NDJSON is deliberate for production logging:

- Agents such as Vector, Fluent Bit, and Datadog can parse entries as a stream
  with bounded memory instead of waiting for a closing array bracket.
- Append-only output needs no array brackets, commas, whole-file rewrites, or
  trailing-comma coordination. Each event is submitted as one complete encoded
  line. `JsonLayer` serializes writer creation and all partial writes for one
  record, so concurrent events through the same layer cannot interleave.
- A crash or interrupted final write can damage the incomplete last line, while
  previously completed lines remain independently parseable.
- Analytics systems can split large inputs on newline boundaries and process
  independent records in parallel.
- Standard tools work directly on the stream, for example
  `head -n 20 app.log | jq -r '.message'`.

Standard JSON arrays are suited to complete documents; NDJSON retains JSON's
structured fields while providing framing designed for continuous log streams.

## Package scope

`ObservabilityLayer` is one Tower layer for Axum 0.8. The application keeps
control of its `tracing` subscriber, writer, filter, panic recovery, listener,
and deployment policy. `JsonLayer` is composable and never installs global
state itself.

This is an independently maintained crate, not official Axum middleware.

## Requirements and installation

The minimum supported Rust version is 1.97.0. Version 2.0.0 supports the Axum
0.8 release line, including Axum 0.8.0 with only its `matched-path` feature.

```toml
[dependencies]
axum = "0.8"
axum-observability = "2.0.0"
tracing = "0.1.44"
tracing-subscriber = { version = "0.3.23", features = ["env-filter"] }
```

Version 2.0.0 is the clean contract described by this checkout. Version 1
remains available for applications that need its API, defaults, or structured
fields; v2 provides no compatibility aliases for those surfaces.

## GCP setup

When this documentation shows one configuration, it uses GCP. Complete
provider-neutral, GCP, AWS, and Azure configuration examples are available in
[`examples`](examples) and [EXAMPLES.md](EXAMPLES.md).

```rust
use axum::{Router, routing::get};
use axum_observability as obs;
use tracing_subscriber::prelude::*;

let config = obs::ObservabilityConfig::default().with_field_convention(obs::FieldConvention::Gcp);

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

`JsonLayer` writes one complete JSON object per line and holds a record-level
lock across the writer's full `write_all` operation. Keep
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
Without `ObservabilityLayer`, extraction rejects with the public
`MissingRequestContext` error, status 500, and the fixed body
`request context unavailable`. This makes middleware misconfiguration
diagnosable without reflecting request data.

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

Application configuration and tests can validate IDs through
`RequestId::parse`, `FromStr`, or `TryFrom`; invalid values return the public,
non-sensitive `InvalidRequestId` error. `RequestId` has no unchecked public
constructor.

The selected value is available from:

- the validated `RequestId` in the `RequestContext` extractor and request
  extension;
- exactly one canonical configured header before downstream service code runs;
- `request_id()` and `correlation_id()`;
- application events inside an enabled package request span;
- the terminal access record; and
- the configured response header, unless disabled.

Configuration can change the request/response header name, disable the response
header, broaden or narrow caller-input validation within Axum's native text
header boundary, or supply a fallible generator. A custom validator can admit
broader RFC 9110 field content such as `id:42`, internal space or tab, and
values longer than 128 bytes. Edge whitespace, controls, non-text bytes, and
values Axum cannot re-emit exactly are rejected before the callback. Generated
values use `RequestId` and retain the baseline grammar. The custom validator is
never applied to generated values. A generator is invoked exactly twice unless
its first result succeeds before the package-owned fallback is used, and
callback failure never produces an invalid ID or alters traffic.

`traceparent` parsing rejects duplicates, uppercase hexadecimal, zero trace or
parent IDs, invalid framing and flags, and unsafe native field content. Version
`00` must use its exact framing. Versions `01` through `fe` validate the known
prefix and treat a delimiter plus any remaining extension bytes as opaque,
including a trailing delimiter, without a package-invented length ceiling.
W3C Trace Context Level 1 is the default; select `TraceContextLevel::Level2`
explicitly when Level 2 key grammar and version-`00` random trace-ID flag
projection are required. Higher versions preserve sampling without assigning
meaning to the random bit. Repeated `tracestate` values are combined in wire
order and accepted only when their selected-level grammar, unique-key rule, and
32-member limit pass. The crate can propagate at least 512 characters and
admits a valid 513-character value; 512 is not a package rejection ceiling.
Empty members are retained and count toward the member limit. An invalid
`tracestate` is discarded without invalidating a valid `traceparent`.

The provider-neutral [`examples/basic.rs`](examples/basic.rs) uses
`ObservabilityConfig::default()` for its Level 1 executable. To enable Level 2,
follow `level_2_config()`, which calls
`with_trace_context_level(TraceContextLevel::Level2)`. Its native example test
sends flags `03` through both configurations and verifies that only Level 2
emits `trace_id_random`.

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
`parent_id`, `trace_flags`, and `trace_sampled`. Configured Level 2 additionally
adds `trace_id_random` for version `00`; Level 1 and higher versions omit it.

The terminal record always has `message = "request completed"`. Its common
semantic fields are:

| Field | JSON type | Presence and meaning |
| --- | --- | --- |
| `request_id` | string | Always; the validated or generated request ID |
| `correlation_id` | string | Always; trace ID when valid W3C context exists, otherwise request ID |
| `trace_id`, `parent_id` | string | Only with valid W3C context |
| `trace_flags` | string | Only with valid W3C context; exactly two lowercase hexadecimal characters |
| `trace_sampled` | boolean | Only with valid W3C context |
| `trace_id_random` | boolean | Only with valid W3C context in configured Level 2 mode |
| `method` | string | Always; HTTP method |
| `path_template` | string | When Axum's `MatchedPath` is available |
| `path` | string | Only with `with_raw_path(true)`; query-free concrete path |
| `operation_id` | string | When an `OperationId` request or response extension exists |
| `status` | number | When a response status is known |
| `duration_ms` | number | Always; non-negative handling and streaming time |
| `peer_ip` | string | Only with the `peer-ip` feature, `with_peer_ip(true)`, and `ConnectInfo` |
| `user_agent` | string | Only with `with_user_agent(true)` and one UTF-8 RFC 9110 field-content value |
| `terminal_reason` | string | Only for `body_error`, `service_error`, or `response_dropped` |

Optional values are omitted; the formatter does not emit `null` placeholders.
Normal completion omits `terminal_reason`; abnormal records do not invent an
`error` summary when the original failure is unavailable. The default level is
`ERROR` for every abnormal terminal reason or a normal 5xx, `WARN` for a
normal 4xx, and `INFO` otherwise. The status-level mapper applies only to
normal completion. Application events cannot replace package correlation,
envelope, provider, or access-catalog fields. Access enrichment cannot replace
terminal access fields either; package-owned fields win. Application `error`
fields remain native application data and are not synthesized on access lines.

`path_template` is the default low-cardinality aggregation key. Concrete `path`
can have unbounded cardinality and may contain identifying data, so it is off by
default.

Axum 0.8 `MatchedPath` already uses the portable whole-segment `{name}` and
terminal `{*name}` forms. Different concrete parameter or catch-all values
therefore retain one template, and unmatched routes omit route and operation
identity rather than substituting the request path.

## Operation IDs

An outer Tower layer cannot inspect request extensions inserted after the
request has been consumed by route middleware. Route-specific operation IDs
should therefore use Axum's native response-extension path:

```rust
use axum::{Extension, http::StatusCode};
use axum_observability::OperationId;

async fn list_items() -> (Extension<OperationId>, StatusCode) {
    (Extension(OperationId::from_static("list-items")), StatusCode::OK)
}
```

An `OperationId` already present before the request reaches observability is
also supported. A response extension takes precedence because it is closest to
the selected route.

## Field conventions

Select one convention on the shared `ObservabilityConfig`, finalize that value,
then construct `json_layer` and the terminal middleware from the same unchanged
configuration. `json_layer` snapshots the convention at construction time;
later builder calls create a different configuration and do not update an
existing layer.

- `Generic` is the provider-neutral default and uses `level`.
- `Gcp` replaces `level` with `severity`, adds
  `logging.googleapis.com/trace`,
  `logging.googleapis.com/trace_sampled`, and a structured `httpRequest` access
  object. `httpRequest` maps enabled `path`, `peer_ip`, and `user_agent` to
  `requestUrl`, `remoteIp`, and `userAgent`; method, status, and latency use
  `requestMethod`, numeric `status`, and a seconds string. The trace field is
  the bare validated 32-character W3C trace ID. The crate never emits a fake
  `logging.googleapis.com/spanId`. Selecting `Gcp` resolves to the newest GCP
  profile implemented by the installed crate, currently `0.1.0`; use
  `with_gcp_profile_version(GcpProfileVersion::V0_1_0)` for an exact pin.
- `Aws` adds `xray_trace_id` in `1-8hex-24hex` form. Selecting it resolves to
  exact current profile `0.1.0`; use
  `with_aws_profile_version(AwsProfileVersion::V0_1_0)` for an exact pin and
  `aws_profile_version()` for introspection. Other dynamic pins fail to parse.
  It does not create an X-Ray segment or parse `X-Amzn-Trace-Id`.
- `Azure` adds `operation_Id` and `operation_ParentId`. Selecting it resolves
  to exact current profile `0.1.0`; use
  `with_azure_profile_version(AzureProfileVersion::V0_1_0)` for an exact pin and
  `azure_profile_version()` for introspection. Other dynamic pins fail to
  parse. It does not initialize an Azure SDK or parse legacy `Request-Id`
  headers.

Provider trace fields are omitted without valid W3C context and never change
which request metadata is captured. They correlate logs; trace creation,
sampling policy, and export remain application concerns.
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
  `response_dropped` record at `ERROR`; and
- once the guard completes, later polling or drop cannot emit a duplicate.

Status and duration reflect the latest trustworthy state. If the response was
never produced, status is omitted. The monotonic clock is saturating, so a bad
custom clock cannot produce a negative duration.

Custom generator, validator, level-mapper, access-enricher, and clock panics
are contained with safe fallback behavior. An initial clock failure uses the
package monotonic clock; a finish-time failure falls back to the request start.
This containment requires Rust's default `panic = "unwind"`; Rust code cannot
recover from `panic = "abort"`.
Formatter serialization and writer errors do not replace the HTTP response.
Writer failures can still mean a log record was not delivered; applications
remain responsible for choosing and monitoring the output destination.

## Configuration

`ObservabilityConfig` is builder-based:

| Method | Default | Purpose |
| --- | --- | --- |
| `with_field_convention` | `FieldConvention::Generic` | Select one provider field convention |
| `with_gcp_profile_version` | latest supported GCP version | Select and pin an exact GCP profile version |
| `with_aws_profile_version` | current AWS profile | Select and pin exact AWS profile `0.1.0` |
| `with_azure_profile_version` | current Azure profile | Select and pin exact Azure profile `0.1.0` |
| `with_trace_context_level` | `TraceContextLevel::Level1` | Select W3C Trace Context Level 1 or Level 2 |
| `with_request_id_header` | `x-request-id` | Set the request and response correlation header |
| `with_response_header` | `true` | Enable or disable response-header injection |
| `with_raw_path` | `false` | Opt into query-free concrete path capture |
| `with_peer_ip` | `false` | With the `peer-ip` feature, opt into trusted socket-peer capture |
| `with_user_agent` | `false` | Opt into one unambiguous text User-Agent value |
| `with_request_id_generator` | random 128-bit ID | Supply a fallible typed generator, invoked up to twice per replacement |
| `with_request_id_validator` | accepts baseline | Broaden or narrow caller IDs within Axum's native text-header boundary |
| `with_status_level_mapper` | 5xx/4xx/other mapping | Map final status to a `tracing::Level` |
| `with_clock` | `Instant::now` | Supply a monotonic clock, primarily for deterministic tests |
| `with_access_enricher` | no extra fields | Add synchronous application-owned terminal fields |

`gcp_profile_version()`, `aws_profile_version()`,
`azure_profile_version()`, and `trace_context_level()` expose resolved
non-secret settings for diagnostics and conformance evidence. Unsupported
dynamic profile pins fail when parsed as their typed version; no network lookup
is performed.

Unknown options do not exist: configuration is compile-time checked. The header
setter accepts a validated `http::HeaderName`; use `HeaderName::from_static` or
`HeaderName::try_from` at the configuration boundary:

```rust
use axum::http::HeaderName;
use axum_observability::ObservabilityConfig;

let config = ObservabilityConfig::default()
    .with_request_id_header(HeaderName::from_static("x-correlation-id"));
# let _: ObservabilityConfig = config;
```

Enrichment values must be safe to log; the crate does not redact
application-owned fields.

## Proxy trust and privacy

`peer_ip` comes only from Axum `ConnectInfo<SocketAddr>` when the `peer-ip`
feature and runtime opt-in are both enabled. The crate never
parses `Forwarded` or `X-Forwarded-For`, because trusting caller-controlled
forwarding headers without a known proxy boundary permits spoofing. Configure
trusted proxy handling before constructing `ConnectInfo` if the original client
address is required.

Raw paths, peer IPs, and User-Agent values are independently off by default;
enabling any of them changes the application's privacy posture. The terminal
schema never logs query strings, request or response bodies,
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
| `peer_ip` is absent | The feature or runtime opt-in is disabled, or no `ConnectInfo<SocketAddr>` exists | Enable `peer-ip`, call `with_peer_ip(true)`, and provide a trusted peer extension |
| Caller request ID is replaced | It is missing, duplicate, invalid, or rejected by custom policy | Send one baseline value, or align the caller value with the configured validator |
| GCP trace link is absent | `traceparent` is missing or invalid | Send one valid lowercase W3C `traceparent`; do not provide a project-qualified value |
| Duplicate framework access lines | Another access logger remains enabled | Disable the competing access logger when this crate owns terminal records |

## Compatibility and development

The crate supports Rust 1.97.0 or newer and the Axum 0.8 release line. The
public `ObservabilityService` is the nameable Tower service produced by
`ObservabilityLayer`. In the 1.x release line, exported APIs, configuration
defaults, structured fields, and supported runtime versions are compatibility
contracts. Breaking changes require a new major version, explicit changelog
coverage, and migration guidance.

The current Unreleased schema and callback changes are reserved for `2.0.0`;
see the changelog migration section before upgrading a 1.x application.
Version 2 exposes no v1 constructor aliases or compatibility shims. Build both
the middleware and JSON formatter from one `ObservabilityConfig`.

Development uses [just](https://github.com/casey/just). The normal gates are:

```bash
brew install rust llvm actionlint zizmor
```

The Homebrew Rust and LLVM versions must match. The coverage recipes detect an
active Homebrew Rust compiler and select Homebrew's `llvm-cov` and
`llvm-profdata` automatically.

```bash
just qa
```

`just qa` runs formatting, Clippy with warnings denied, tests, doctests,
dependency policy, the RustSec audit, [actionlint](https://github.com/rhysd/actionlint),
and [zizmor](https://docs.zizmor.sh/). Maintainers should follow the public
[release architecture and guide](RELEASE.md).

## Property and mutation testing

Stable property tests generate valid W3C trace context and exercise equivalent
multi-header `tracestate` layouts as part of the normal test suite. Mutation
testing remains an explicit maintainer campaign:

```bash
just mutation
```

Mutation testing runs outside `just qa`. Add a behavioral test when a surviving
mutant exposes a real contract gap. Equivalent transformations do not need
artificial assertions.

## References

- [Axum middleware](https://docs.rs/axum/0.8/axum/middleware/index.html)
  documents middleware placement and ordering.
- [`Extension`](https://docs.rs/axum/0.8/axum/struct.Extension.html) and
  [`IntoResponseParts`](https://docs.rs/axum/0.8/axum/response/trait.IntoResponseParts.html)
  document request and response extensions.
- [`http-body::Body`](https://docs.rs/http-body/1/http_body/trait.Body.html)
  defines frame polling, EOF, and size-hint behavior.
- [`tracing-subscriber::Layer`](https://docs.rs/tracing-subscriber/0.3.23/tracing_subscriber/layer/trait.Layer.html)
  and [`EnvFilter`](https://docs.rs/tracing-subscriber/0.3.23/tracing_subscriber/filter/struct.EnvFilter.html)
  define formatter composition and filtering.
- [W3C Trace Context Level 1 Recommendation](https://www.w3.org/TR/2021/REC-trace-context-1-20211123/)
  defines the default `traceparent` and `tracestate` contract.
- [W3C Trace Context Level 2 Candidate Recommendation Draft](https://www.w3.org/TR/2024/CRD-trace-context-2-20240328/)
  defines the explicit Level 2 key grammar and random trace-ID flag.
- [Google Cloud trace and log integration](https://cloud.google.com/trace/docs/trace-log-integration)
  documents the bare trace ID as the preferred trace field format.
- [Google Cloud Trace release notes](https://cloud.google.com/trace/docs/release-notes)
  record when the bare trace ID became the preferred form while the full
  project resource name remained supported.
- [Google Cloud structured logging](https://cloud.google.com/logging/docs/structured-logging)
  documents `severity`, `message`, `httpRequest`, and special trace fields.
- [AWS X-Ray trace IDs](https://docs.aws.amazon.com/xray/latest/devguide/xray-api-sendingdata.html#xray-api-traceids)
  documents conversion from W3C to `1-8hex-24hex` form.
- [Azure Application Insights data model](https://learn.microsoft.com/en-us/azure/azure-monitor/app/data-model-complete)
  defines `operation_Id` as the root-operation identifier and
  `operation_ParentId` as the immediate-parent identifier.

## License

[MIT](LICENSE)
