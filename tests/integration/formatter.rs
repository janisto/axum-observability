use super::*;

#[tokio::test(flavor = "current_thread")]
async fn formatter_preserves_typed_application_fields_and_background_events() {
    let config = ObservabilityConfig::default();
    let capture = Capture::default();
    let _guard = subscriber(&config, capture.clone()).set_default();
    let error = io::Error::other("controlled application error");
    tracing::info!(
        floating = 1.25_f64,
        signed = -7_i64,
        unsigned = 9_u64,
        ready = true,
        error = &error as &dyn std::error::Error,
        severity = "spoofed",
        "background event"
    );

    let records = capture.records();
    let record = &records[0];
    assert_eq!(record["message"], "background event");
    assert_eq!(record["floating"], 1.25);
    assert_eq!(record["signed"], -7);
    assert_eq!(record["unsigned"], 9);
    assert_eq!(record["ready"], true);
    assert_eq!(record["error"], "controlled application error");
    assert_eq!(record["level"], "INFO");
    assert!(record.get("severity").is_none());
    assert!(record["timestamp"].is_string());
    assert!(record.get("request_id").is_none());
}

#[test]
fn formatter_writes_each_ndjson_event_once() {
    let config = ObservabilityConfig::default();
    let capture = CountingCapture::default();
    let _guard = tracing_subscriber::registry()
        .with(config.json_layer(capture.clone()))
        .set_default();

    tracing::info!(answer = 42_u64, ready = true, "buffered event");

    assert_eq!(capture.writes.load(Ordering::SeqCst), 1);
    let records = capture.capture.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["message"], "buffered event");
    assert_eq!(records[0]["answer"], 42);
}

#[tokio::test(flavor = "current_thread")]
async fn immediate_writer_failure_does_not_change_the_http_result() {
    let writer = FailingCapture {
        state: Arc::new(Mutex::new(FailingWriterState::default())),
        prefix_len: 0,
    };
    let config = ObservabilityConfig::default();
    let layer = config.json_layer(writer.clone()).log_internal_errors(false);
    let _guard = tracing_subscriber::registry().with(layer).set_default();
    let app = Router::new()
        .route("/", get(|| async { (StatusCode::OK, "ok") }))
        .layer(ObservabilityLayer::new(config));

    let response = app
        .oneshot(Request::new(Body::empty()))
        .await
        .expect("writer failure must not replace the response");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(response.into_body(), 1_024).await.expect("body"),
        "ok"
    );
    let state = writer.state.lock().expect("failing writer lock");
    assert_eq!(state.calls, 1);
    assert!(state.bytes.is_empty());
}

#[test]
fn partial_writer_failure_is_not_retried() {
    let writer = FailingCapture {
        state: Arc::new(Mutex::new(FailingWriterState::default())),
        prefix_len: 5,
    };
    let layer = ObservabilityConfig::default()
        .json_layer(writer.clone())
        .log_internal_errors(false);
    let _guard = tracing_subscriber::registry().with(layer).set_default();

    tracing::info!(
        private_value = "must-not-enter-diagnostics",
        "partial write"
    );

    let state = writer.state.lock().expect("failing writer lock");
    assert_eq!(state.calls, 2);
    assert_eq!(state.bytes.len(), 5);
}

#[test]
fn gcp_uses_canonical_cloud_logging_severity_names() {
    let config = ObservabilityConfig::default().with_field_convention(FieldConvention::Gcp);
    let capture = Capture::default();
    let _guard = subscriber(&config, capture.clone()).set_default();

    tracing::trace!("trace event");
    tracing::debug!("debug event");
    tracing::info!("info event");
    tracing::warn!("warn event");
    tracing::error!("error event");

    let records = capture.records();
    let severities = records
        .iter()
        .map(|record| record["severity"].as_str().expect("severity"))
        .collect::<Vec<_>>();
    assert_eq!(severities, ["DEBUG", "DEBUG", "INFO", "WARNING", "ERROR"]);
}

#[tokio::test(flavor = "current_thread")]
async fn gcp_health_route_emits_correlated_application_and_terminal_records() {
    let (status, body, records) = gcp_health_records(LevelFilter::DEBUG).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "ok");
    assert_eq!(records.len(), 3);
    assert_eq!(records[0]["severity"], "INFO");
    assert_eq!(records[0]["message"], "health check");
    assert_eq!(records[0]["service_name"], "example-service");
    assert_eq!(records[0]["service_version"], "0.3.0");
    assert_eq!(records[0]["health_status"], "ok");
    assert_eq!(records[1]["severity"], "DEBUG");
    assert_eq!(records[1]["message"], "dependency check");
    assert_eq!(records[1]["dependency"], "database");
    assert_eq!(records[1]["dependency_status"], "ok");
    assert_eq!(records[1]["check_duration_ms"], 3);
    assert!(records[1]["check_duration_ms"].is_u64());

    for record in &records {
        assert_eq!(record["request_id"], "health-example");
        assert_eq!(record["correlation_id"], "health-example");
    }
    let terminal = &records[2];
    assert_eq!(terminal["severity"], "INFO");
    assert_eq!(terminal["message"], "request completed");
    assert_eq!(terminal["path_template"], "/health");
    assert_eq!(terminal["httpRequest"]["requestMethod"], "GET");
    assert_eq!(terminal["httpRequest"]["requestUrl"], "/health");
    assert_eq!(terminal["httpRequest"]["status"], 200);
    assert!(terminal["httpRequest"]["status"].is_u64());
    assert!(terminal["httpRequest"]["latency"].is_string());
    for application_only in [
        "service_name",
        "service_version",
        "health_status",
        "dependency",
        "dependency_status",
        "check_duration_ms",
    ] {
        assert!(terminal.get(application_only).is_none());
    }
    assert_eq!(
        records
            .iter()
            .filter(|record| record["message"] == "request completed")
            .count(),
        1
    );
}

#[tokio::test(flavor = "current_thread")]
async fn gcp_health_route_respects_info_filter() {
    let (status, body, records) = gcp_health_records(LevelFilter::INFO).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "ok");
    assert_eq!(records.len(), 2);
    assert_eq!(records[0]["message"], "health check");
    assert_eq!(records[1]["message"], "request completed");
    assert!(
        records
            .iter()
            .all(|record| record["request_id"] == "health-example")
    );
    let serialized = serde_json::to_string(&records).expect("records serialize");
    assert!(!serialized.contains("dependency check"));
    assert!(!serialized.contains("check_duration_ms"));
}
