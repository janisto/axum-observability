//! Azure-oriented structured logging setup.

use axum::{Router, routing::get};
use axum_observability::{ObservabilityConfig, ObservabilityLayer, Preset};
use tracing_subscriber::prelude::*;

fn main() {
    let config = ObservabilityConfig::default().with_preset(Preset::Azure);
    tracing_subscriber::registry()
        .with(config.json_layer(std::io::stdout))
        .init();

    let _app: Router = Router::new()
        .route("/health", get(|| async { "ok" }))
        .layer(ObservabilityLayer::new(config));
}
