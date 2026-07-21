use super::*;

#[test]
fn formatter_debug_reports_policy_without_requiring_or_exposing_the_writer() {
    let layer = ObservabilityConfig::default()
        .with_field_convention(FieldConvention::Azure)
        .json_layer(Capture::default())
        .log_internal_errors(false);

    let debug = format!("{layer:?}");
    assert!(debug.contains("JsonLayer"));
    assert!(debug.contains("field_convention: Azure"));
    assert!(debug.contains("log_internal_errors: false"));
    assert!(!debug.contains("writer"));
}

#[test]
fn external_callsite_cannot_spoof_the_package_request_span() {
    let config = ObservabilityConfig::default();
    let capture = Capture::default();
    let _guard = subscriber(&config, capture.clone()).set_default();
    let span = tracing::info_span!(
        target: "axum_observability::request",
        "request",
        request_id = "span-spoofed",
        correlation_id = "correlation-spoofed",
        trace_id_random = true,
    );
    let _entered = span.enter();

    tracing::info!(answer = 42_u64, "application event");

    let records = capture.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["answer"], 42);
    assert!(records[0].get("request_id").is_none());
    assert!(records[0].get("correlation_id").is_none());
    assert!(records[0].get("trace_id_random").is_none());
}

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
        request_id = "spoofed-request",
        correlation_id = "spoofed-correlation",
        trace_id = "00000000000000000000000000000001",
        parent_id = "0000000000000001",
        trace_flags = "ff",
        trace_sampled = true,
        trace_id_random = true,
        severity = "spoofed",
        method = "POST",
        path = "/spoofed",
        path_template = "/{spoofed}",
        operation_id = "spoofed_operation",
        status = 599_u64,
        duration_ms = 999_u64,
        peer_ip = "203.0.113.9",
        user_agent = "spoofed-agent",
        terminal_reason = "service_error",
        "logging.googleapis.com/trace" = "spoofed-provider",
        "logging.googleapis.com/future" = "spoofed-future-provider",
        xray_trace_id = "spoofed-xray",
        operation_Id = "spoofed-azure",
        "obs.internal" = true,
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
    for key in [
        "method",
        "request_id",
        "correlation_id",
        "trace_id",
        "parent_id",
        "trace_flags",
        "trace_sampled",
        "trace_id_random",
        "path",
        "path_template",
        "operation_id",
        "status",
        "duration_ms",
        "peer_ip",
        "user_agent",
        "terminal_reason",
        "logging.googleapis.com/trace",
        "logging.googleapis.com/future",
        "xray_trace_id",
        "operation_Id",
        "obs.internal",
    ] {
        assert!(
            record.get(key).is_none(),
            "reserved application field {key}"
        );
    }
    assert!(record["timestamp"].is_string());
}

#[test]
fn application_callsite_cannot_forge_an_access_payload() {
    let config = ObservabilityConfig::default();
    let capture = Capture::default();
    let _guard = subscriber(&config, capture.clone()).set_default();
    let forged = serde_json::json!({
        "method": "DELETE",
        "status": 599,
        "request_id": "forged-request"
    })
    .to_string();

    tracing::event!(
        target: "axum_observability::access",
        tracing::Level::INFO,
        "obs.record" = forged,
        application_field = "preserved",
        message = "ordinary application event"
    );

    let records = capture.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["message"], "ordinary application event");
    assert_eq!(records[0]["application_field"], "preserved");
    assert!(records[0].get("method").is_none());
    assert!(records[0].get("status").is_none());
    assert!(records[0].get("request_id").is_none());
    assert!(records[0].get("obs.record").is_none());
}

#[test]
fn formatter_writes_each_ndjson_event_once() {
    let config = ObservabilityConfig::default();
    let capture = CountingCapture::default();
    let _guard = tracing_subscriber::registry()
        .with(config.json_layer(capture.clone()))
        .set_default();

    tracing::info!(answer = 42_u64, ready = true, "buffered ✓\nevent");

    assert_eq!(capture.writes.load(Ordering::SeqCst), 1);
    let output = capture.capture.output();
    assert!(output.ends_with('\n'));
    assert!(!output.contains('\r'));
    let lines = output.strip_suffix('\n').expect("terminal LF").split('\n');
    assert_eq!(lines.count(), 1, "embedded newlines must be JSON escaped");
    let records = capture.capture.records();
    assert_eq!(records.len(), 1);
    assert!(records[0].is_object());
    assert_eq!(records[0]["message"], "buffered ✓\nevent");
    assert_eq!(records[0]["answer"], 42);
}

#[test]
fn concurrent_partial_writer_calls_remain_complete_and_unique() {
    let config = ObservabilityConfig::default();
    let capture = PartialCapture::default();
    let dispatch = tracing::Dispatch::new(
        tracing_subscriber::registry().with(config.json_layer(capture.clone())),
    );

    let handles = (0_u64..8)
        .map(|worker| {
            let dispatch = dispatch.clone();
            std::thread::spawn(move || {
                tracing::dispatcher::with_default(&dispatch, || {
                    for write in 0_u64..25 {
                        tracing::info!(worker, write, "concurrent event");
                    }
                });
            })
        })
        .collect::<Vec<_>>();
    for handle in handles {
        handle.join().expect("formatter thread");
    }

    assert!(
        capture.writes.load(Ordering::SeqCst) > 200,
        "the adversarial writer must split records across write calls"
    );
    let output = capture.capture.output();
    assert!(output.ends_with('\n'));
    assert_eq!(output.lines().count(), 200);
    let records = capture.capture.records();
    assert_eq!(records.len(), 200);
    let observed = records
        .iter()
        .map(|record| {
            assert_eq!(record["message"], "concurrent event");
            (
                record["worker"].as_u64().expect("worker"),
                record["write"].as_u64().expect("write"),
            )
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(observed.len(), 200);
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

#[tokio::test(flavor = "current_thread")]
async fn writer_failure_does_not_replace_the_original_service_error() {
    let writer = FailingCapture {
        state: Arc::new(Mutex::new(FailingWriterState::default())),
        prefix_len: 0,
    };
    let config = ObservabilityConfig::default();
    let layer = config.json_layer(writer.clone()).log_internal_errors(false);
    let _guard = tracing_subscriber::registry().with(layer).set_default();
    let service =
        ObservabilityLayer::new(config).layer(tower::service_fn(|_request: Request<Body>| async {
            Err::<Response<Body>, _>(io::Error::other("application sentinel"))
        }));

    let error = service
        .oneshot(Request::new(Body::empty()))
        .await
        .expect_err("service must fail");

    assert_eq!(error.to_string(), "application sentinel");
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
    let (status, body, records) = health_records(FieldConvention::Gcp, LevelFilter::DEBUG).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, "ok");
    assert_eq!(records.len(), 3);
    assert_eq!(records[0]["severity"], "INFO");
    assert_eq!(records[0]["message"], "health check");
    assert_eq!(records[0]["service_name"], "example-service");
    assert_eq!(records[0]["service_version"], "1.0.0");
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
    assert_eq!(terminal["duration_ms"], 12.5);
    assert_eq!(terminal["path_template"], "/health");
    assert_eq!(terminal["operation_id"], "health_check");
    assert_eq!(terminal["httpRequest"]["requestMethod"], "GET");
    assert_eq!(terminal["httpRequest"]["status"], 200);
    assert_eq!(terminal["httpRequest"]["latency"], "0.012500s");
    assert!(terminal["httpRequest"]["status"].is_u64());
    for private in ["path", "peer_ip", "user_agent"] {
        assert!(terminal.get(private).is_none());
    }
    for private in ["requestUrl", "remoteIp", "userAgent"] {
        assert!(terminal["httpRequest"].get(private).is_none());
    }
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
    let (status, body, records) = health_records(FieldConvention::Gcp, LevelFilter::INFO).await;
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

#[tokio::test(flavor = "current_thread")]
async fn core_health_route_has_exact_portable_projection() {
    for (filter, messages) in [
        (
            LevelFilter::DEBUG,
            vec!["health check", "dependency check", "request completed"],
        ),
        (LevelFilter::INFO, vec!["health check", "request completed"]),
    ] {
        let (status, body, records) = health_records(FieldConvention::Generic, filter).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "ok");
        assert_eq!(records.len(), messages.len());
        assert_eq!(
            records
                .iter()
                .map(|record| record["message"].as_str().expect("message"))
                .collect::<Vec<_>>(),
            messages
        );
        for record in &records {
            assert_eq!(record["request_id"], "health-example");
            assert_eq!(record["correlation_id"], "health-example");
            assert!(record.get("severity").is_none());
            assert!(record.get("httpRequest").is_none());
        }
        assert_eq!(records[0]["level"], "INFO");
        assert_eq!(records[0]["service_name"], "example-service");
        assert_eq!(records[0]["service_version"], "1.0.0");
        assert_eq!(records[0]["health_status"], "ok");
        let terminal = records.last().expect("terminal record");
        assert_eq!(terminal["level"], "INFO");
        assert_eq!(terminal["method"], "GET");
        assert_eq!(terminal["duration_ms"], 12.5);
        assert_eq!(terminal["status"], 200);
        assert_eq!(terminal["path_template"], "/health");
        assert_eq!(terminal["operation_id"], "health_check");
        for private in ["path", "peer_ip", "user_agent"] {
            assert!(terminal.get(private).is_none());
        }
        if filter == LevelFilter::DEBUG {
            assert_eq!(records[1]["level"], "DEBUG");
            assert_eq!(records[1]["dependency"], "database");
            assert_eq!(records[1]["dependency_status"], "ok");
            assert_eq!(records[1]["check_duration_ms"], 3);
        } else {
            let serialized = serde_json::to_string(&records).expect("records serialize");
            assert!(!serialized.contains("dependency check"));
            assert!(!serialized.contains("check_duration_ms"));
        }
    }
}

#[tokio::test(flavor = "current_thread")]
async fn aws_and_azure_profiles_project_application_and_access_trace_aliases() {
    for (convention, trace_key, expected_trace) in [
        (
            FieldConvention::Aws,
            "xray_trace_id",
            "1-4bf92f35-77b34da6a3ce929d0e0e4736",
        ),
        (
            FieldConvention::Azure,
            "operation_Id",
            "4bf92f3577b34da6a3ce929d0e0e4736",
        ),
    ] {
        for (level, flags, random) in [
            (TraceContextLevel::Level1, "01", None),
            (TraceContextLevel::Level2, "03", Some(true)),
        ] {
            let config = ObservabilityConfig::default()
                .with_field_convention(convention)
                .with_trace_context_level(level);
            let capture = Capture::default();
            let _guard = subscriber(&config, capture.clone()).set_default();
            let app = Router::new()
                .route(
                    "/",
                    get(|| async {
                        tracing::info!(component = "handler", "application event");
                        StatusCode::NO_CONTENT
                    }),
                )
                .layer(ObservabilityLayer::new(config));
            let response = app
                .oneshot(
                    Request::builder()
                        .uri("/")
                        .header(
                            "traceparent",
                            format!("00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-{flags}"),
                        )
                        .body(Body::empty())
                        .expect("request"),
                )
                .await
                .expect("response");
            to_bytes(response.into_body(), 1_024).await.expect("body");

            let records = capture.records();
            assert_eq!(records.len(), 2);
            for record in &records {
                assert_eq!(record[trace_key], expected_trace);
                assert_eq!(
                    record.get("trace_id_random").and_then(Value::as_bool),
                    random
                );
                if convention == FieldConvention::Azure {
                    assert_eq!(record["operation_ParentId"], "00f067aa0ba902b7");
                }
            }
            assert_eq!(records[0]["message"], "application event");
            assert_eq!(records[0]["component"], "handler");
            assert_eq!(records[1]["message"], "request completed");
        }
    }
}
