use super::*;

#[tokio::test(flavor = "current_thread")]
async fn valid_trace_context_correlates_application_and_access_events_without_spoofing() {
    async fn handler(context: RequestContext) -> impl IntoResponse {
        tracing::info!(request_id = "spoofed", answer = 42_u64, "application event");
        assert_eq!(context.correlation_id(), "4bf92f3577b34da6a3ce929d0e0e4736");
        StatusCode::NO_CONTENT
    }

    let config = ObservabilityConfig::default().with_field_convention(FieldConvention::Gcp);
    let capture = Capture::default();
    let _guard = subscriber(&config, capture.clone()).set_default();
    let app = Router::new()
        .route("/items/{id}", get(handler))
        .layer(ObservabilityLayer::new(config));
    let request = Request::builder()
        .uri("/items/42")
        .header("x-request-id", "request-42")
        .header(
            "traceparent",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
        )
        .header("tracestate", "vendor=value")
        .body(Body::empty())
        .expect("request");

    let response = app.oneshot(request).await.expect("response");
    to_bytes(response.into_body(), 1_024).await.expect("body");
    let records = capture.records();
    assert_eq!(records.len(), 2);
    for record in &records {
        assert_eq!(record["request_id"], "request-42");
        assert_eq!(record["correlation_id"], "4bf92f3577b34da6a3ce929d0e0e4736");
        assert_eq!(record["severity"], "INFO");
        assert_eq!(
            record["logging.googleapis.com/trace"],
            "4bf92f3577b34da6a3ce929d0e0e4736"
        );
        assert_eq!(record["logging.googleapis.com/trace_sampled"], true);
    }
    assert_eq!(records[0]["message"], "application event");
    assert_eq!(records[0]["answer"], 42);
    assert!(records[0].get("level").is_none());
    assert!(records[1]["httpRequest"].is_object());
    assert_eq!(records[1]["httpRequest"]["requestMethod"], "GET");
    assert!(records[1]["httpRequest"].get("requestUrl").is_none());
    assert!(records[1]["httpRequest"]["status"].is_u64());
}

#[tokio::test(flavor = "current_thread")]
async fn future_traceparent_extension_is_accepted_but_never_logged() {
    let config = ObservabilityConfig::default();
    let capture = Capture::default();
    let _guard = subscriber(&config, capture.clone()).set_default();
    let app = Router::new()
        .route("/", get(context_handler))
        .layer(ObservabilityLayer::new(config));
    let request = Request::builder()
        .uri("/")
        .header("x-request-id", "future-request")
        .header(
            "traceparent",
            "01-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01-opaque-secret",
        )
        .body(Body::empty())
        .expect("request");

    let response = app.oneshot(request).await.expect("response");
    let body = to_bytes(response.into_body(), 1_024).await.expect("body");
    assert_eq!(body, "future-request|4bf92f3577b34da6a3ce929d0e0e4736");
    let output = serde_json::to_string(&capture.records()).expect("records serialize");
    assert!(!output.contains("opaque-secret"));
}

#[tokio::test(flavor = "current_thread")]
async fn duplicate_traceparent_is_untrusted_and_falls_back_to_request_id() {
    let config = ObservabilityConfig::default();
    let capture = Capture::default();
    let _guard = subscriber(&config, capture).set_default();
    let app = Router::new()
        .route("/", get(context_handler))
        .layer(ObservabilityLayer::new(config));
    let mut request = Request::builder()
        .uri("/")
        .header("x-request-id", "request-only")
        .body(Body::empty())
        .expect("request");
    for value in [
        "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
        "00-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa-bbbbbbbbbbbbbbbb-00",
    ] {
        request
            .headers_mut()
            .append("traceparent", HeaderValue::from_str(value).expect("header"));
    }
    let response = app.oneshot(request).await.expect("response");
    let body = to_bytes(response.into_body(), 1_024).await.expect("body");
    assert_eq!(body, "request-only|request-only");
}
