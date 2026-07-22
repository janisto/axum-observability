use std::{collections::BTreeMap, env, error::Error, sync::Arc};

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode, header::AUTHORIZATION},
    response::{IntoResponse, Response},
    routing::get,
};
use axum_observability::{
    FieldConvention, ObservabilityConfig, ObservabilityLayer, RequestContext, TraceContextLevel,
};
use serde::Serialize;
use serde_json::json;
use tokio::net::TcpListener;
use tracing_subscriber::prelude::*;

#[derive(Clone)]
struct AppState {
    canary: Arc<str>,
}

#[derive(Serialize)]
struct TraceBody<'a> {
    ok: bool,
    request_id: &'a str,
    canary_received: bool,
}

#[derive(Serialize)]
struct ErrorBody {
    error: &'static str,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let case = required_environment("OBS_E2E_CASE")?;
    let canary = required_environment("OBS_E2E_SECRET_CANARY")?;
    let port = configured_port()?;
    let config = configured_case(&case)?;
    tracing_subscriber::registry()
        .with(config.json_layer(std::io::stdout))
        .init();

    let app = Router::new()
        .route("/trace", get(trace))
        .with_state(AppState {
            canary: Arc::from(canary),
        })
        .layer(ObservabilityLayer::new(config));
    let listener = TcpListener::bind(("0.0.0.0", port)).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn trace(
    context: RequestContext,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    let authorized = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.strip_prefix("Bearer ") == Some(state.canary.as_ref()));
    if !authorized {
        return (
            StatusCode::UNAUTHORIZED,
            Json(ErrorBody {
                error: "unauthorized",
            }),
        )
            .into_response();
    }
    tracing::info!(event = "trace", "handler");
    Json(TraceBody {
        ok: true,
        request_id: context.request_id().as_str(),
        canary_received: true,
    })
    .into_response()
}

fn configured_case(name: &str) -> Result<ObservabilityConfig, Box<dyn Error>> {
    let level_one =
        ObservabilityConfig::default().with_trace_context_level(TraceContextLevel::Level1);
    let config = match name {
        "common_level1" => level_one,
        "common_level2" => {
            ObservabilityConfig::default().with_trace_context_level(TraceContextLevel::Level2)
        }
        "aws_level1" => level_one.with_field_convention(FieldConvention::Aws),
        "azure_level1" => level_one.with_field_convention(FieldConvention::Azure),
        "gcp_level1" => level_one
            .with_field_convention(FieldConvention::Gcp)
            .with_access_enricher(|_| {
                BTreeMap::from([(
                    "e2e_configuration".to_owned(),
                    json!({
                        "system_id": "sys-402",
                        "server_settings": {
                            "nodes": [{
                                "hostname": "srv-01",
                                "port": 8080,
                                "ssl_enabled": true
                            }]
                        }
                    }),
                )])
            }),
        _ => return Err("OBS_E2E_CASE must select one supported E2E case".into()),
    };
    Ok(config)
}

fn required_environment(name: &str) -> Result<String, Box<dyn Error>> {
    let value = env::var(name)?;
    if value.is_empty() {
        return Err(format!("{name} must be nonempty").into());
    }
    Ok(value)
}

fn configured_port() -> Result<u16, Box<dyn Error>> {
    let raw = env::var("PORT").unwrap_or_else(|_| "8080".to_owned());
    let port = raw.parse::<u16>()?;
    if port == 0 {
        return Err("PORT must be between 1 and 65535".into());
    }
    Ok(port)
}
