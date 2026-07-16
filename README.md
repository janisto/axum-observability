# axum-observability

Axum middleware for request IDs, W3C trace correlation, request-scoped
structured events, and terminal access logs.

The crate is deliberately small: it does not install an OpenTelemetry SDK,
create spans for a tracing backend, export metrics, ship logs, or own a cloud
logging client. Your application keeps control of its `tracing` subscriber,
writer, filtering, and backend.

## Install

```toml
[dependencies]
axum-observability = "0.2.0"
```

The minimum supported Rust version is 1.96.1. The v0.x API may evolve between
minor versions; patch releases preserve the documented behavior.

## Quick start: Google Cloud JSON

```rust
use axum::{Router, routing::get};
use axum_observability as obs;
use tracing_subscriber::prelude::*;

let config = obs::ObservabilityConfig::default().with_preset(obs::Preset::Gcp);

tracing_subscriber::registry()
    .with(tracing_subscriber::EnvFilter::from_default_env())
    .with(config.json_layer(std::io::stdout))
    .init();

let app = Router::new()
    .route("/health", get(|| async { "ok" }))
    .layer(obs::ObservabilityLayer::new(config));
# let _: Router = app;
```

`JsonLayer` writes one JSON object per line. It is a normal composable
`tracing-subscriber` layer and never initializes global state itself.

Handlers can extract the validated context directly:

```rust
use axum_observability::RequestContext;

async fn handler(context: RequestContext) {
    tracing::info!(item_count = 3, "loaded items");
    assert!(!context.request_id().is_empty());
}
```

Application events emitted inside the request inherit the package correlation
fields. Event fields cannot overwrite those reserved values.

## Middleware order

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
    // routes and application middleware
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

This order is covered by integration tests for recovered panics and timeouts.
The terminal record is emitted only when the response body reaches EOF, errors,
or is dropped—not when response headers are created.

## Request IDs and trace context

The default header is `X-Request-ID`. Exactly one incoming value is accepted
when it contains 1–128 ASCII URI-unreserved characters: letters, digits, `-`,
`.`, `_`, and `~`. Missing, empty, duplicate, oversized, non-ASCII, or otherwise
invalid values are replaced. The fallback is a random 128-bit value encoded as
32 lowercase hexadecimal characters. The selected ID is returned on the
response unless disabled.

Configuration can change the header name, disable the response header, narrow
validation, supply a fallible generator, customize status levels, provide a
monotonic clock seam, and add controlled access fields. Generated values still
pass the baseline policy; generator failure is contained by the package-owned
fallback.

`traceparent` parsing rejects duplicates, uppercase hexadecimal, zero trace or
parent IDs, invalid framing and flags, and invalid version lengths. Valid
`tracestate` values are combined in wire order with unique keys, at most 32
members, and at most 512 bytes. Invalid `tracestate` is discarded without
invalidating a valid `traceparent`.

With valid trace context, `correlation_id` is the trace ID; otherwise it is the
request ID. The crate records inbound context only. It does not invent a local
span ID, mutate outbound headers, or claim the incoming parent belongs to this
service.

## Access record

Every terminal record has `message = "request completed"`, `method`, escaped
`path` without the query, final `status` when known, and non-negative
`duration_ms`. It also includes these values when available:

- `path_template` from Axum `MatchedPath`;
- `operation_id` from an explicit `OperationId` request or response extension;
- `remote_ip` from `ConnectInfo<SocketAddr>` only;
- one unambiguous raw `user_agent`;
- `terminal_reason` and controlled `error` for body errors, service errors, or
  early response drop;
- `request_id`, `correlation_id`, and validated W3C trace fields.

The default level mapping is `ERROR` for 5xx, `WARN` for 4xx, and `INFO` for all
other statuses.

Because observability wraps route middleware, route-specific operation IDs
should be returned as an Axum response extension so the outer layer can observe
them:

```rust
use axum::{Extension, http::StatusCode};
use axum_observability::OperationId;

async fn list_items() -> (Extension<OperationId>, StatusCode) {
    (Extension(OperationId::new("list-items")), StatusCode::OK)
}
```

An `OperationId` already present on the request before it reaches the
observability layer remains supported. A response extension takes precedence.

## Presets

- `Default` emits provider-neutral fields and `level`.
- `Gcp` emits `severity`, `logging.googleapis.com/trace`,
  `logging.googleapis.com/trace_sampled`, and a real nested `httpRequest`
  object with method, safe path, status, latency, peer IP, and user agent.
- `Aws` adds `xray_trace_id` in `1-8hex-24hex` form.
- `Azure` adds `operation_Id` and `operation_ParentId`.

Provider correlation fields are present only for validated W3C context. The GCP
trace field is always the bare validated 32-character trace ID. The crate never
prepends `projects/{project}/traces/`; the bare trace ID is Google Cloud's
current preferred format and matches the sibling package contract.

## Privacy boundary

The crate never logs query strings, request or response bodies, cookies,
authorization values, arbitrary headers, or forwarded-IP headers. A concrete
escaped path is logged, and the GCP request URL uses that same safe path.
Applications remain responsible for ensuring their own event fields and access
enrichment contain no credentials, personal data, or secrets.

See [EXAMPLES.md](EXAMPLES.md) for all presets and configuration examples.

## Development

```bash
just qa
just package-check
```

Mutation and fuzz campaigns are explicit: `just mutation` and
`just fuzz <request_id|traceparent|tracestate> <seconds>`. See
[RELEASE.md](RELEASE.md) for the release gate. Fuzzing requires a Rust nightly
toolchain; the crate's build and MSRV remain on stable Rust 1.96.1.

## License

[MIT](LICENSE)
