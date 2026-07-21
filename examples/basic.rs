//! Provider-neutral setup.

use axum::{Router, routing::get};
use axum_observability::{ObservabilityConfig, ObservabilityLayer, TraceContextLevel};
use tracing_subscriber::prelude::*;

fn default_config() -> ObservabilityConfig {
    ObservabilityConfig::default()
}

/// Return the explicit W3C Trace Context Level 2 configuration.
pub fn level_2_config() -> ObservabilityConfig {
    ObservabilityConfig::default().with_trace_context_level(TraceContextLevel::Level2)
}

fn basic_app(config: ObservabilityConfig) -> Router {
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .layer(ObservabilityLayer::new(config))
}

fn main() {
    let config = default_config();
    tracing_subscriber::registry()
        .with(config.json_layer(std::io::stdout))
        .init();

    let _app = basic_app(config);
}

#[cfg(test)]
mod tests {
    use std::{
        io,
        sync::{Arc, Mutex},
    };

    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use serde_json::Value;
    use tower::ServiceExt as _;
    use tracing_subscriber::prelude::*;

    use super::{basic_app, default_config, level_2_config};

    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Vec<u8>>>);

    struct CaptureWriter(Arc<Mutex<Vec<u8>>>);

    impl io::Write for CaptureWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0
                .lock()
                .expect("capture lock")
                .extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'writer> tracing_subscriber::fmt::MakeWriter<'writer> for Capture {
        type Writer = CaptureWriter;

        fn make_writer(&'writer self) -> Self::Writer {
            CaptureWriter(self.0.clone())
        }
    }

    impl Capture {
        fn records(&self) -> Vec<Value> {
            let output = self.0.lock().expect("capture lock").clone();
            String::from_utf8(output)
                .expect("JSON is UTF-8")
                .lines()
                .map(|line| serde_json::from_str(line).expect("line is JSON"))
                .collect()
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn basic_examples_demonstrate_distinct_trace_context_levels() {
        for (name, config, expected_random) in [
            ("Level 1 default", default_config(), None),
            ("Level 2", level_2_config(), Some(true)),
        ] {
            let capture = Capture::default();
            let _guard = tracing_subscriber::registry()
                .with(config.json_layer(capture.clone()))
                .set_default();
            let request = Request::builder()
                .uri("/health")
                .header("x-request-id", "trace-level-example")
                .header(
                    "traceparent",
                    "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-03",
                )
                .body(Body::empty())
                .expect("request");
            let response = basic_app(config).oneshot(request).await.expect("response");
            to_bytes(response.into_body(), 1_024).await.expect("body");

            let records = capture.records();
            assert_eq!(records.len(), 1, "{name}");
            let record = &records[0];
            assert_eq!(record["trace_flags"], "03", "{name}");
            assert_eq!(record["trace_sampled"], true, "{name}");
            assert_eq!(
                record.get("trace_id_random").and_then(Value::as_bool),
                expected_random,
                "{name}"
            );
        }
    }
}
