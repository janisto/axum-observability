# Examples

This guide shows how to wire `axum-observability` into Axum services while
keeping one log contract across Google Cloud, provider-neutral, AWS, and Azure
deployments.

When one configuration is shown, this project uses GCP as the canonical
example. The other examples remain first-class and are compiled by the test
suite.

| Example | Purpose |
| --- | --- |
| [`examples/gcp.rs`](examples/gcp.rs) | Canonical Google Cloud Logging field shape. |
| [`examples/basic.rs`](examples/basic.rs) | Generic JSON for local or provider-neutral pipelines. |
| [`examples/aws.rs`](examples/aws.rs) | CloudWatch-friendly JSON and a derived X-Ray trace ID. |
| [`examples/azure.rs`](examples/azure.rs) | Azure Monitor and Application Insights operation fields. |

## Core wiring

Every service follows the same shape:

1. Create one `ObservabilityConfig` and select its field convention.
2. Install `config.json_layer(writer)` on the application's existing
   `tracing-subscriber` registry.
3. Add `ObservabilityLayer` outside response-producing middleware so it can
   observe the final status and response body.
4. Use ordinary `tracing` events in handlers and services.

The canonical GCP wiring is:

```rust
use axum::{Router, routing::get};
use axum_observability::{ObservabilityConfig, ObservabilityLayer, FieldConvention};
use tracing_subscriber::prelude::*;

let config = ObservabilityConfig::default().with_field_convention(FieldConvention::Gcp);

tracing_subscriber::registry()
    .with(config.json_layer(std::io::stdout))
    .init();

let app = Router::new()
    .route("/health", get(|| async { "ok" }))
    .layer(ObservabilityLayer::new(config));
# let _: Router = app;
```

No Google Cloud project ID is required. With valid W3C context,
`logging.googleapis.com/trace` is the exact bare 32-character trace ID.

## Check the canonical GCP configuration

```bash
cargo run --quiet --example gcp
```

The example makes one in-process `/health` request without binding a listener.
Its stdout is exactly two correlated application events followed by one
terminal request event, each as one JSON object. The examples are also compiled
by `cargo test --all-targets`.

## Expected request behavior

For a request carrying:

```text
X-Request-ID: demo-123
traceparent: 00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01
tracestate: vendor=value
```

the request ID remains `demo-123`, while `correlation_id` becomes the W3C trace
ID. Handler events and the terminal access record share the correlation fields
when the request span is enabled. The access record also includes a structured
`httpRequest` object.

Representative GCP fields:

```json
{"severity":"INFO","message":"request completed","request_id":"demo-123","correlation_id":"4bf92f3577b34da6a3ce929d0e0e4736","trace_id":"4bf92f3577b34da6a3ce929d0e0e4736","logging.googleapis.com/trace":"4bf92f3577b34da6a3ce929d0e0e4736","logging.googleapis.com/trace_sampled":true,"method":"GET","path_template":"/health","operation_id":"health_check","status":200,"httpRequest":{"requestMethod":"GET","status":200}}
```

The crate does not create spans for a tracing backend and therefore does not
manufacture `logging.googleapis.com/spanId` from the incoming parent ID.
Raw path, direct peer IP, and user agent capture are independent opt-ins and
remain absent in this default example; GCP selection does not enable them.

## Provider-neutral JSON

```bash
cargo check --locked --example basic
```

The default convention writes `level` and generic correlation fields without
provider-specific aliases.

## AWS

```bash
cargo check --locked --example aws
```

The AWS convention keeps flat JSON. A valid W3C trace ID is also formatted as
`xray_trace_id`, for example
`1-4bf92f35-77b34da6a3ce929d0e0e4736`. The crate does not create X-Ray segments
or parse `X-Amzn-Trace-Id`.

## Azure

```bash
cargo check --locked --example azure
```

The Azure convention maps valid W3C values to `operation_Id` and
`operation_ParentId`. It does not initialize an Azure SDK or parse legacy
`Request-Id` headers.

## Application logging

Use `tracing` directly in handlers and services. Events emitted while the
request span is enabled inherit the package's validated request and trace
correlation without passing a logger or context through application APIs:

```rust
let item_id = "item-42";
tracing::info!(item_id = %item_id, "loading item");

let error = std::io::Error::other("backend unavailable");
tracing::error!(error = %error, item_id = %item_id, "item load failed");
```

For operation spans, skip arguments by default and allowlist only fields that
are safe and useful to record:

```rust
#[tracing::instrument(skip_all, fields(item_id = %item_id), err)]
async fn validate_item(item_id: &str) -> Result<(), &'static str> {
    if item_id.is_empty() {
        Err("item ID is empty")
    } else {
        Ok(())
    }
}
```

Generic application-local level wrappers are unnecessary in Rust and can hide
useful `tracing` callsite metadata. Prefer direct events or application-local
semantic macros for specific domain events.

## Per-project checklist

- Use Rust 1.97.0 or newer.
- Use GCP when documentation needs one representative configuration.
- Initialize the subscriber once at process startup, never in request code or a
  library.
- Keep `ObservabilityLayer` outside panic recovery, timeout, CORS, and body-limit
  middleware whose final responses must be recorded.
- Keep `axum_observability::request=info` enabled when application events need
  request-span correlation; terminal access records carry their own correlation
  fields even when that span is filtered.
- Group logs by `path_template`, not the concrete request path.
- Configure trusted proxy handling before inserting `ConnectInfo<SocketAddr>`;
  the crate never trusts forwarded headers itself.
- Keep provider tracing SDKs separate from this correlation crate.
- Never place secrets or raw bodies in application log or enrichment fields.
- Run `just qa` and `cargo publish --dry-run --locked` before release.

## References

- [Axum middleware](https://docs.rs/axum/latest/axum/middleware/index.html)
- [tracing-subscriber filtering](https://docs.rs/tracing-subscriber/latest/tracing_subscriber/filter/struct.EnvFilter.html)
- [Google Cloud: Link log entries with traces](https://docs.cloud.google.com/trace/docs/trace-log-integration)
- [Google Cloud structured logging](https://docs.cloud.google.com/logging/docs/structured-logging)
- [W3C Trace Context](https://www.w3.org/TR/trace-context/)
