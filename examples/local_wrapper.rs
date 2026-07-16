//! Application-owned configuration wrapper.

use axum::{Router, routing::get};
use axum_observability::{ObservabilityConfig, ObservabilityLayer, Preset};
use tracing_subscriber::prelude::*;

fn local_observability() -> ObservabilityConfig {
    ObservabilityConfig::default()
        .with_preset(Preset::Default)
        .with_request_id_header("x-correlation-id")
        .expect("static header name")
        .with_request_id_validator(|value| value.starts_with("local-"))
}

fn main() {
    let config = local_observability();
    tracing_subscriber::registry()
        .with(config.json_layer(std::io::stdout))
        .init();

    let _app: Router = Router::new()
        .route("/health", get(|| async { "ok" }))
        .layer(ObservabilityLayer::new(config));
}
