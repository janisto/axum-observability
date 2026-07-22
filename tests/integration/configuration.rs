use super::*;

#[test]
fn configuration_debug_reports_safe_policies_without_callback_internals() {
    let generator_secret = "generator-secret-must-not-leak".to_owned();
    let config = ObservabilityConfig::default()
        .with_field_convention(FieldConvention::Aws)
        .with_trace_context_level(TraceContextLevel::Level2)
        .with_request_id_header(HeaderName::from_static("x-correlation-id"))
        .with_response_header(false)
        .with_raw_path(true)
        .with_user_agent(true)
        .with_request_id_generator(move || {
            let _ = &generator_secret;
            None
        });
    #[cfg(feature = "peer-ip")]
    let config = config.with_peer_ip(true);

    let debug = format!("{config:?}");
    for expected in [
        "ObservabilityConfig",
        "field_convention: Aws",
        "trace_context_level: Level2",
        "request_id_header: \"x-correlation-id\"",
        "response_header: false",
        "raw_path: true",
        "user_agent: true",
    ] {
        assert!(
            debug.contains(expected),
            "missing {expected:?} from {debug:?}"
        );
    }
    #[cfg(feature = "peer-ip")]
    assert!(debug.contains("peer_ip: true"));
    for callback in [
        "generator",
        "validator",
        "level_mapper",
        "clock",
        "enricher",
    ] {
        assert!(
            !debug.contains(callback),
            "leaked {callback:?} in {debug:?}"
        );
    }
    assert!(!debug.contains("generator-secret-must-not-leak"));
}

#[test]
fn trace_context_level_defaults_to_one_and_can_be_selected_explicitly() {
    let level_one = ObservabilityConfig::default();
    assert_eq!(level_one.trace_context_level(), TraceContextLevel::Level1);

    let level_two = level_one.with_trace_context_level(TraceContextLevel::Level2);
    assert_eq!(level_two.trace_context_level(), TraceContextLevel::Level2);
}

#[tokio::test(flavor = "current_thread")]
async fn custom_header_validator_and_response_header_configuration_are_effective() {
    let config = ObservabilityConfig::default()
        .with_request_id_header(HeaderName::from_static("x-correlation-id"))
        .with_request_id_validator(|value| value.starts_with("custom-"))
        .with_request_id_generator(|| {
            Some(RequestId::parse("custom-generated").expect("valid generated ID"))
        });
    assert!(HeaderName::try_from("not a header").is_err());
    let capture = Capture::default();
    let _guard = subscriber(&config, capture).set_default();
    let app = Router::new()
        .route("/", get(custom_identity_handler))
        .layer(ObservabilityLayer::new(config));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/")
                .header("x-correlation-id", "baseline-valid-but-custom-invalid")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.headers()["x-correlation-id"], "custom-generated");
    assert!(response.headers().get("x-request-id").is_none());
    let body = to_bytes(response.into_body(), 1_024).await.expect("body");
    assert_eq!(body, "custom-generated|1|true");

    let disabled = ObservabilityConfig::default().with_response_header(false);
    let capture = Capture::default();
    let _guard = subscriber(&disabled, capture).set_default();
    let app = Router::new()
        .route("/", get(canonical_identity_handler))
        .layer(ObservabilityLayer::new(disabled));
    let response = app
        .oneshot(Request::new(Body::empty()))
        .await
        .expect("response");
    assert!(response.headers().get("x-request-id").is_none());
    let body = to_bytes(response.into_body(), 1_024).await.expect("body");
    let body = String::from_utf8(body.to_vec()).expect("context body");
    let fields = body.split('|').collect::<Vec<_>>();
    assert_eq!(fields.len(), 3);
    let request_id = fields[0];
    assert_eq!(fields[1], "1");
    assert_eq!(fields[2], request_id);
    assert!(RequestId::parse(request_id).is_ok());
}

#[tokio::test(flavor = "current_thread")]
async fn custom_level_clock_enrichment_and_operation_id_preserve_reserved_fields() {
    let origin = Instant::now();
    let calls = Arc::new(AtomicUsize::new(0));
    let clock_calls = calls.clone();
    let config = ObservabilityConfig::default()
        .with_status_level_mapper(|_| tracing::Level::ERROR)
        .with_clock(move || {
            if clock_calls.fetch_add(1, Ordering::SeqCst) == 0 {
                origin
            } else {
                origin + Duration::from_millis(1_500)
            }
        })
        .with_access_enricher(|context| {
            BTreeMap::from([
                ("tenant".to_owned(), Value::String("public".to_owned())),
                ("status".to_owned(), Value::from(999)),
                ("target".to_owned(), Value::String("spoofed".to_owned())),
                (
                    "logging.googleapis.com/trace".to_owned(),
                    Value::String("spoofed-provider".to_owned()),
                ),
                (
                    "logging.googleapis.com/spanId".to_owned(),
                    Value::String("application-span".to_owned()),
                ),
                (
                    "logging.googleapis.com/labels".to_owned(),
                    serde_json::json!({"component": "worker"}),
                ),
                ("obs.internal".to_owned(), Value::Bool(true)),
                ("_obs_internal".to_owned(), Value::Bool(true)),
                (
                    "remote_ip".to_owned(),
                    Value::String("203.0.113.10".to_owned()),
                ),
                (
                    "request_id".to_owned(),
                    Value::String(format!("spoofed-{}", context.request_id())),
                ),
            ])
        });
    let capture = Capture::default();
    let _guard = subscriber(&config, capture.clone()).set_default();
    let app = Router::new()
        .route("/", get(|| async { StatusCode::OK }))
        .layer(ObservabilityLayer::new(config));
    let mut request = Request::builder()
        .uri("/")
        .header("x-request-id", "real-request")
        .body(Body::empty())
        .expect("request");
    request
        .extensions_mut()
        .insert(OperationId::from_static("list-items\nvariant"));
    let response = app.oneshot(request).await.expect("response");
    to_bytes(response.into_body(), 1_024).await.expect("body");

    let records = capture.records();
    let record = &records[0];
    assert_eq!(record["level"], "ERROR");
    assert_eq!(record["duration_ms"], 1_500);
    assert!(record["duration_ms"].is_u64());
    assert_eq!(record["tenant"], "public");
    assert_eq!(record["status"], 200);
    assert_eq!(record["target"], "axum_observability::access");
    assert_eq!(record["request_id"], "real-request");
    assert_eq!(record["operation_id"], "list-items\nvariant");
    assert_eq!(record["logging.googleapis.com/trace"], "spoofed-provider");
    assert_eq!(record["logging.googleapis.com/spanId"], "application-span");
    assert_eq!(
        record["logging.googleapis.com/labels"]["component"],
        "worker"
    );
    assert_eq!(record["obs.internal"], true);
    assert_eq!(record["_obs_internal"], true);
    assert_eq!(record["remote_ip"], "203.0.113.10");
}

#[tokio::test(flavor = "current_thread")]
async fn response_operation_id_overrides_preseeded_request_operation_id() {
    let config = ObservabilityConfig::default();
    let capture = Capture::default();
    let _guard = subscriber(&config, capture.clone()).set_default();
    let app = Router::new()
        .route(
            "/",
            get(|| async {
                (
                    Extension(OperationId::from_static("route-list-items")),
                    StatusCode::NO_CONTENT,
                )
            }),
        )
        .layer(ObservabilityLayer::new(config));
    let mut request = Request::new(Body::empty());
    request
        .extensions_mut()
        .insert(OperationId::from_static("preseeded-operation"));

    app.oneshot(request).await.expect("response");

    let records = capture.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["operation_id"], "route-list-items");
}
