use super::*;

#[tokio::test(flavor = "current_thread")]
async fn route_identity_is_canonical_stable_and_omits_unmatched_metadata() {
    let config = ObservabilityConfig::default();
    let capture = Capture::default();
    let _guard = subscriber(&config, capture.clone()).set_default();
    let long_name = "a".repeat(65);
    let long_route = format!("/long/{{{long_name}}}");
    let app = Router::new()
        .route(
            "/items/{item_id}",
            get(|| async {
                (
                    Extension(OperationId::from_static("get_item")),
                    StatusCode::OK,
                )
            }),
        )
        .route(
            "/files/{*path}",
            get(|| async {
                (
                    Extension(OperationId::from_static("get_file")),
                    StatusCode::OK,
                )
            }),
        )
        .route(
            &long_route,
            get(|| async {
                (
                    Extension(OperationId::from_static("get_long")),
                    StatusCode::OK,
                )
            }),
        )
        .route(
            "/literal*star",
            get(|| async {
                (
                    Extension(OperationId::from_static("get_literal_star")),
                    StatusCode::OK,
                )
            }),
        )
        .layer(ObservabilityLayer::new(config));

    for uri in [
        "/items/tenant-a",
        "/items/tenant-b",
        "/long/value",
        "/literal*star",
        "/files/tenant-a/one",
        "/files/tenant-b/two",
        "/missing/private-value",
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        to_bytes(response.into_body(), 1_024).await.expect("body");
    }

    let records = capture.records();
    assert_eq!(records.len(), 7);
    for record in &records[0..2] {
        assert_eq!(record["path_template"], "/items/{item_id}");
        assert_eq!(record["operation_id"], "get_item");
    }
    assert_eq!(records[2]["path_template"], long_route);
    assert_eq!(records[2]["operation_id"], "get_long");
    assert_eq!(records[3]["path_template"], "/literal*star");
    assert_eq!(records[3]["operation_id"], "get_literal_star");
    for record in &records[4..6] {
        assert_eq!(record["path_template"], "/files/{*path}");
        assert_eq!(record["operation_id"], "get_file");
    }
    assert!(records[6].get("path_template").is_none());
    assert!(records[6].get("operation_id").is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn field_conventions_emit_exact_provider_trace_shapes() {
    for (convention, key, expected) in [
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
        let config = ObservabilityConfig::default().with_field_convention(convention);
        let capture = Capture::default();
        let _guard = subscriber(&config, capture.clone()).set_default();
        let app = Router::new()
            .route("/", get(|| async { StatusCode::OK }))
            .layer(ObservabilityLayer::new(config));
        let request = Request::builder()
            .uri("/")
            .header(
                "traceparent",
                "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
            )
            .body(Body::empty())
            .expect("request");
        let response = app.oneshot(request).await.expect("response");
        to_bytes(response.into_body(), 1_024).await.expect("body");
        let records = capture.records();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0][key], expected);
        if convention == FieldConvention::Azure {
            assert_eq!(records[0]["operation_ParentId"], "00f067aa0ba902b7");
        }
    }
}

#[tokio::test(flavor = "current_thread")]
#[cfg(feature = "peer-ip")]
async fn field_conventions_emit_exact_non_conflicting_access_schemas() {
    let common = BTreeSet::from([
        "correlation_id",
        "duration_ms",
        "level",
        "message",
        "method",
        "operation_id",
        "parent_id",
        "path",
        "path_template",
        "peer_ip",
        "request_id",
        "status",
        "target",
        "tenant",
        "timestamp",
        "trace_flags",
        "trace_id",
        "trace_sampled",
        "user_agent",
    ]);

    for convention in [
        FieldConvention::Generic,
        FieldConvention::Gcp,
        FieldConvention::Aws,
        FieldConvention::Azure,
    ] {
        let record = representative_access_record(convention).await;
        let mut expected = common.clone();
        match convention {
            FieldConvention::Generic => {}
            FieldConvention::Gcp => {
                expected.remove("level");
                expected.extend([
                    "httpRequest",
                    "logging.googleapis.com/trace",
                    "logging.googleapis.com/trace_sampled",
                    "severity",
                ]);
            }
            FieldConvention::Aws => {
                expected.insert("xray_trace_id");
            }
            FieldConvention::Azure => {
                expected.extend(["operation_Id", "operation_ParentId"]);
            }
            _ => unreachable!("all current conventions are covered"),
        }
        let actual = record
            .as_object()
            .expect("access record object")
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        assert_eq!(actual, expected, "unexpected {convention:?} schema");
        assert!(record["status"].is_u64());
        assert!(record["duration_ms"].is_number());
        assert_eq!(record["trace_flags"], "01");
        assert!(record.get("trace_id_random").is_none());
        assert_eq!(record["path"], "/items/42");
        assert!(!record.to_string().contains("secret=query"));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn response_abandonment_adds_only_its_documented_terminal_field() {
    let config = ObservabilityConfig::default();
    let capture = Capture::default();
    let _guard = subscriber(&config, capture.clone()).set_default();
    let app = Router::new()
        .route("/items/{id}", get(|| async { "item" }))
        .layer(ObservabilityLayer::new(config));

    let completed = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/items/1")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    to_bytes(completed.into_body(), 1_024).await.expect("body");

    let abandoned = app
        .oneshot(
            Request::builder()
                .uri("/items/2")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    drop(abandoned);

    let records = capture.records();
    assert_eq!(records.len(), 2);
    let normal_keys = records[0]
        .as_object()
        .expect("normal object")
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut abnormal_keys = records[1]
        .as_object()
        .expect("abnormal object")
        .keys()
        .cloned()
        .collect::<BTreeSet<_>>();
    assert!(abnormal_keys.remove("terminal_reason"));
    assert_eq!(abnormal_keys, normal_keys);
    assert_eq!(records[1]["terminal_reason"], "response_dropped");
    assert!(records[1].get("error").is_none());
}

#[test]
fn invalid_request_span_trace_ids_do_not_emit_aws_metadata() {
    let config = ObservabilityConfig::default().with_field_convention(FieldConvention::Aws);
    let capture = Capture::default();
    let _guard = subscriber(&config, capture.clone()).set_default();
    let invalid = [
        "short".to_owned(),
        "abc".to_owned(),
        "g".repeat(32),
        "0".repeat(32),
    ];
    for trace_id in &invalid {
        let span = tracing::info_span!(
            target: "axum_observability::request",
            "request",
            trace_id = trace_id.as_str(),
        );
        span.in_scope(|| tracing::info!("application event"));
    }

    let records = capture.records();
    assert_eq!(records.len(), invalid.len());
    for (record, trace_id) in records.iter().zip(invalid) {
        assert_eq!(record["trace_id"], trace_id);
        assert!(record.get("xray_trace_id").is_none());
    }
}

#[tokio::test(flavor = "current_thread")]
async fn filtered_request_span_does_not_remove_access_record_correlation() {
    let config = ObservabilityConfig::default().with_field_convention(FieldConvention::Gcp);
    let capture = Capture::default();
    let filtered = config
        .json_layer(capture.clone())
        .with_filter(Targets::new().with_default(LevelFilter::WARN));
    let _guard = tracing_subscriber::registry().with(filtered).set_default();
    let app = Router::new()
        .route("/", get(|| async { StatusCode::INTERNAL_SERVER_ERROR }))
        .layer(ObservabilityLayer::new(config));
    let request = Request::builder()
        .uri("/")
        .header("x-request-id", "request-500")
        .header(
            "traceparent",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
        )
        .body(Body::empty())
        .expect("request");

    app.oneshot(request).await.expect("response");

    let records = capture.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["severity"], "ERROR");
    assert_eq!(records[0]["request_id"], "request-500");
    assert_eq!(
        records[0]["correlation_id"],
        "4bf92f3577b34da6a3ce929d0e0e4736"
    );
    assert_eq!(
        records[0]["logging.googleapis.com/trace"],
        "4bf92f3577b34da6a3ce929d0e0e4736"
    );
    assert_eq!(records[0]["logging.googleapis.com/trace_sampled"], true);
}

#[tokio::test(flavor = "current_thread")]
async fn request_span_directive_preserves_correlation_and_rejects_late_spoofing() {
    let config = ObservabilityConfig::default().with_field_convention(FieldConvention::Gcp);
    let capture = Capture::default();
    let filtered = config.json_layer(capture.clone()).with_filter(
        Targets::new()
            .with_default(LevelFilter::WARN)
            .with_target("axum_observability::request", LevelFilter::INFO),
    );
    let _guard = tracing_subscriber::registry().with(filtered).set_default();
    let app = Router::new()
        .route(
            "/",
            get(|| async {
                let span = tracing::Span::current();
                span.record("request_id", "spoofed-request");
                span.record("trace_id", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
                tracing::warn!("handler warning");
                StatusCode::INTERNAL_SERVER_ERROR
            }),
        )
        .layer(ObservabilityLayer::new(config));
    let request = Request::builder()
        .uri("/")
        .header("x-request-id", "original-request")
        .header(
            "traceparent",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
        )
        .body(Body::empty())
        .expect("request");

    app.oneshot(request).await.expect("response");

    let records = capture.records();
    assert_eq!(records.len(), 2);
    for record in records {
        assert_eq!(record["request_id"], "original-request");
        assert_eq!(record["trace_id"], "4bf92f3577b34da6a3ce929d0e0e4736");
        assert_eq!(
            record["logging.googleapis.com/trace"],
            "4bf92f3577b34da6a3ce929d0e0e4736"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn synchronous_inner_service_events_are_inside_the_request_span() {
    let config = ObservabilityConfig::default().with_field_convention(FieldConvention::Gcp);
    let capture = Capture::default();
    let _guard = subscriber(&config, capture.clone()).set_default();
    let service = ObservabilityLayer::new(config).layer(SynchronousLoggingService);
    let request = Request::builder()
        .header("x-request-id", "sync-request")
        .header(
            "traceparent",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
        )
        .body(Body::empty())
        .expect("request");

    service.oneshot(request).await.expect("response");

    let records = capture.records();
    let event = records
        .iter()
        .find(|record| record["message"] == "synchronous service call")
        .expect("synchronous event");
    assert_eq!(event["request_id"], "sync-request");
    assert_eq!(event["trace_id"], "4bf92f3577b34da6a3ce929d0e0e4736");
    assert_eq!(
        event["logging.googleapis.com/trace"],
        "4bf92f3577b34da6a3ce929d0e0e4736"
    );
}

#[tokio::test(flavor = "current_thread")]
#[cfg(feature = "peer-ip")]
async fn peer_and_unambiguous_user_agent_are_recorded_without_forwarded_headers() {
    let config = ObservabilityConfig::default()
        .with_peer_ip(true)
        .with_user_agent(true);
    let capture = Capture::default();
    let _guard = subscriber(&config, capture.clone()).set_default();
    let app = Router::new()
        .route("/", get(|| async { StatusCode::OK }))
        .layer(ObservabilityLayer::new(config));
    let mut request = Request::builder()
        .uri("/")
        .header("user-agent", "agent/1")
        .header("x-forwarded-for", "203.0.113.7")
        .body(Body::empty())
        .expect("request");
    request.extensions_mut().insert(ConnectInfo(SocketAddr::new(
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        3210,
    )));
    let response = app.oneshot(request).await.expect("response");
    to_bytes(response.into_body(), 1_024).await.expect("body");
    let records = capture.records();
    assert_eq!(records[0]["peer_ip"], "127.0.0.1");
    assert_eq!(records[0]["user_agent"], "agent/1");
    assert!(records[0].to_string().find("203.0.113.7").is_none());
}

#[tokio::test(flavor = "current_thread")]
#[cfg(feature = "peer-ip")]
async fn identifying_metadata_is_default_off_and_independently_opt_in() {
    let default = privacy_record(ObservabilityConfig::default()).await;
    assert_eq!(default["path_template"], "/items/{id}");
    for field in ["path", "peer_ip", "user_agent"] {
        assert!(default.get(field).is_none(), "default leaked {field}");
    }
    assert!(!default.to_string().contains("query"));
    assert!(!default.to_string().contains("203.0.113.7"));

    let raw_path = privacy_record(ObservabilityConfig::default().with_raw_path(true)).await;
    assert_eq!(raw_path["path"], "/items/42");
    assert!(raw_path.get("peer_ip").is_none());
    assert!(raw_path.get("user_agent").is_none());
    assert!(!raw_path.to_string().contains("secret=query"));

    let peer_ip = privacy_record(ObservabilityConfig::default().with_peer_ip(true)).await;
    assert_eq!(peer_ip["peer_ip"], "127.0.0.1");
    assert!(peer_ip.get("path").is_none());
    assert!(peer_ip.get("user_agent").is_none());
    assert!(!peer_ip.to_string().contains("203.0.113.7"));

    let user_agent = privacy_record(ObservabilityConfig::default().with_user_agent(true)).await;
    assert_eq!(user_agent["user_agent"], "agent/1");
    assert!(user_agent.get("path").is_none());
    assert!(user_agent.get("peer_ip").is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn valid_encoded_paths_are_preserved_and_malformed_escapes_are_omitted() {
    let config = ObservabilityConfig::default().with_raw_path(true);
    let capture = Capture::default();
    let _guard = subscriber(&config, capture.clone()).set_default();
    let app = Router::new()
        .fallback(|| async { StatusCode::NOT_FOUND })
        .layer(ObservabilityLayer::new(config));

    for uri in [
        "/objects/a%2Fb/%E2%9C%93",
        "/objects/bad%2",
        "/objects/bad%GG",
        "/objects/bad%2G",
        "/objects/bad%G2",
        "/a%20%G2",
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(uri)
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        to_bytes(response.into_body(), 1_024).await.expect("body");
    }

    let records = capture.records();
    assert_eq!(records.len(), 6);
    assert_eq!(records[0]["path"], "/objects/a%2Fb/%E2%9C%93");
    for record in &records[1..] {
        assert!(
            record.get("path").is_none(),
            "malformed path was emitted: {record}"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn ambiguous_or_non_text_user_agent_is_omitted_but_htab_is_preserved() {
    let config = ObservabilityConfig::default().with_user_agent(true);
    let capture = Capture::default();
    let _guard = subscriber(&config, capture.clone()).set_default();
    let app = Router::new()
        .route("/", get(|| async { StatusCode::NO_CONTENT }))
        .layer(ObservabilityLayer::new(config));

    let mut duplicate = Request::builder()
        .uri("/")
        .header("user-agent", "first-agent")
        .body(Body::empty())
        .expect("request");
    duplicate
        .headers_mut()
        .append("user-agent", HeaderValue::from_static("second-agent"));
    app.clone().oneshot(duplicate).await.expect("response");

    let request = Request::builder()
        .uri("/")
        .header(
            "user-agent",
            HeaderValue::from_bytes(&[0xff]).expect("opaque header value"),
        )
        .body(Body::empty())
        .expect("request");
    app.clone().oneshot(request).await.expect("response");

    let empty = Request::builder()
        .uri("/")
        .header("user-agent", HeaderValue::from_static(""))
        .body(Body::empty())
        .expect("request");
    app.clone().oneshot(empty).await.expect("response");

    let tab = Request::builder()
        .uri("/")
        .header(
            "user-agent",
            HeaderValue::from_bytes(b"agent/1\tforged").expect("header with tab"),
        )
        .body(Body::empty())
        .expect("request");
    app.oneshot(tab).await.expect("response");

    let records = capture.records();
    assert_eq!(records.len(), 4);
    assert!(
        records[..3]
            .iter()
            .all(|record| record.get("user_agent").is_none())
    );
    assert_eq!(records[3]["user_agent"], "agent/1\tforged");
}

#[tokio::test(flavor = "current_thread")]
async fn enabled_single_text_user_agent_is_recorded() {
    let config = ObservabilityConfig::default().with_user_agent(true);
    let capture = Capture::default();
    let _guard = subscriber(&config, capture.clone()).set_default();
    let app = Router::new()
        .route("/", get(|| async { StatusCode::NO_CONTENT }))
        .layer(ObservabilityLayer::new(config));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/")
                .header("user-agent", "agent/1")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    to_bytes(response.into_body(), 1_024).await.expect("body");

    let records = capture.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["user_agent"], "agent/1");
}

#[tokio::test(flavor = "current_thread")]
async fn default_record_omits_peer_ip() {
    let config = ObservabilityConfig::default();
    let capture = Capture::default();
    let _guard = subscriber(&config, capture.clone()).set_default();
    let app = Router::new()
        .route("/", get(|| async { StatusCode::NO_CONTENT }))
        .layer(ObservabilityLayer::new(config));
    let response = app
        .oneshot(Request::new(Body::empty()))
        .await
        .expect("response");
    to_bytes(response.into_body(), 1_024).await.expect("body");

    let records = capture.records();
    assert_eq!(records.len(), 1);
    assert!(records[0].get("peer_ip").is_none());
}
