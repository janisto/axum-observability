//! Provider-neutral setup.

use axum::{Router, routing::get};
use axum_observability::{ObservabilityConfig, ObservabilityLayer};
use tracing_subscriber::prelude::*;

fn main() {
    let config = ObservabilityConfig::default();
    tracing_subscriber::registry()
        .with(config.json_layer(std::io::stdout))
        .init();

    let _app: Router = Router::new()
        .route("/health", get(|| async { "ok" }))
        .layer(ObservabilityLayer::new(config));
}
