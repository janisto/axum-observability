//! Google Cloud-compatible structured logs from an in-process health route.

use axum::{
    Extension, Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
    response::IntoResponse,
    routing::get,
};
use axum_observability::{FieldConvention, ObservabilityConfig, ObservabilityLayer, OperationId};
use tower::ServiceExt as _;
use tracing_subscriber::{
    Layer as _,
    filter::{LevelFilter, Targets},
    prelude::*,
};

async fn health() -> impl IntoResponse {
    tracing::info!(
        service_name = "example-service",
        service_version = "1.0.0",
        health_status = "ok",
        "health check"
    );
    tracing::debug!(
        dependency = "database",
        dependency_status = "ok",
        check_duration_ms = 3_u64,
        "dependency check"
    );
    (Extension(OperationId::from_static("health_check")), "ok")
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let config = ObservabilityConfig::default().with_field_convention(FieldConvention::Gcp);
    let json = config
        .json_layer(std::io::stdout)
        .with_filter(Targets::new().with_default(LevelFilter::DEBUG));
    let _guard = tracing_subscriber::registry().with(json).set_default();
    let app = Router::new()
        .route("/health", get(health))
        .layer(ObservabilityLayer::new(config));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .header("x-request-id", "health-example")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(response.into_body(), 1_024).await.expect("body"),
        "ok"
    );
}
