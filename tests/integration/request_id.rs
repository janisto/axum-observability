use super::*;

#[tokio::test(flavor = "current_thread")]
async fn accepts_one_valid_request_id_and_returns_it_on_the_response() {
    let config = ObservabilityConfig::default();
    let capture = Capture::default();
    let _guard = subscriber(&config, capture.clone()).set_default();
    let app = Router::new()
        .route("/context", get(canonical_identity_handler))
        .layer(ObservabilityLayer::new(config));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/context?secret=never-log-this")
                .header("x-request-id", "safe_ID~1")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.headers()["x-request-id"], "safe_ID~1");
    assert!(capture.records().is_empty(), "body has not completed yet");

    let body = to_bytes(response.into_body(), 1_024).await.expect("body");
    assert_eq!(body, "safe_ID~1|1|safe_ID~1");
    let records = capture.records();
    assert_eq!(records.len(), 1);
    assert!(records[0].get("path").is_none());
    assert_eq!(records[0]["path_template"], "/context");
    assert!(records[0].to_string().find("secret").is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn canonical_request_id_is_shared_by_context_header_span_response_and_terminal_record() {
    let config = ObservabilityConfig::default();
    let capture = Capture::default();
    let _guard = subscriber(&config, capture.clone()).set_default();
    let app = Router::new()
        .route("/", get(canonical_logging_handler))
        .layer(ObservabilityLayer::new(config));

    let response = app
        .oneshot(
            Request::builder()
                .uri("/")
                .header("x-request-id", "same-everywhere")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.headers()["x-request-id"], "same-everywhere");
    let body = to_bytes(response.into_body(), 1_024).await.expect("body");
    assert_eq!(body, "same-everywhere|1|same-everywhere");

    let records = capture.records();
    assert_eq!(records.len(), 2);
    for record in records {
        assert_eq!(record["request_id"], "same-everywhere");
        assert_eq!(record["correlation_id"], "same-everywhere");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn missing_invalid_duplicate_and_comma_joined_ids_are_canonicalized_before_service() {
    let config = ObservabilityConfig::default().with_request_id_generator(|| {
        Some(RequestId::parse("generated-canonical").expect("valid generated ID"))
    });
    let capture = Capture::default();
    let _guard = subscriber(&config, capture.clone()).set_default();
    let app = Router::new()
        .route("/", get(canonical_identity_handler))
        .layer(ObservabilityLayer::new(config));

    let mut requests = vec![
        Request::builder()
            .uri("/")
            .body(Body::empty())
            .expect("missing request ID"),
        Request::builder()
            .uri("/")
            .header("x-request-id", "rejected/secret")
            .body(Body::empty())
            .expect("invalid request ID"),
        Request::builder()
            .uri("/")
            .header("x-request-id", "first,second")
            .body(Body::empty())
            .expect("joined request ID"),
    ];
    let mut duplicate = Request::builder()
        .uri("/")
        .header("x-request-id", "first-secret")
        .body(Body::empty())
        .expect("duplicate request ID");
    duplicate
        .headers_mut()
        .append("x-request-id", HeaderValue::from_static("second-secret"));
    requests.push(duplicate);

    for request in requests {
        let response = app.clone().oneshot(request).await.expect("response");
        assert_eq!(response.headers()["x-request-id"], "generated-canonical");
        let body = to_bytes(response.into_body(), 1_024).await.expect("body");
        assert_eq!(body, "generated-canonical|1|generated-canonical");
    }

    let output = serde_json::to_string(&capture.records()).expect("records serialize");
    for rejected in [
        "rejected/secret",
        "first,second",
        "first-secret",
        "second-secret",
    ] {
        assert!(!output.contains(rejected), "leaked {rejected:?}");
    }
}

#[tokio::test(flavor = "current_thread")]
async fn replaces_duplicate_ids_and_invokes_the_generator_once() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let generator_attempts = attempts.clone();
    let config = ObservabilityConfig::default().with_request_id_generator(move || {
        generator_attempts.fetch_add(1, Ordering::SeqCst);
        Some(RequestId::parse("generated-once").expect("valid generated ID"))
    });
    let capture = Capture::default();
    let _guard = subscriber(&config, capture).set_default();
    let app = Router::new()
        .route("/", get(context_handler))
        .layer(ObservabilityLayer::new(config));
    let mut request = Request::builder()
        .uri("/")
        .body(Body::empty())
        .expect("request");
    request
        .headers_mut()
        .append("x-request-id", HeaderValue::from_static("first"));
    request
        .headers_mut()
        .append("x-request-id", HeaderValue::from_static("second"));

    let response = app.oneshot(request).await.expect("response");
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    assert_eq!(response.headers()["x-request-id"], "generated-once");
}

#[tokio::test(flavor = "current_thread")]
async fn generator_failure_is_retried_then_falls_back_without_applying_custom_validator() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let generator_attempts = attempts.clone();
    let config = ObservabilityConfig::default()
        .with_request_id_generator(move || {
            generator_attempts.fetch_add(1, Ordering::SeqCst);
            panic!("generator failure");
        })
        .with_request_id_validator(|_| false);
    let capture = Capture::default();
    let _guard = subscriber(&config, capture).set_default();
    let app = Router::new()
        .route("/", get(context_handler))
        .layer(ObservabilityLayer::new(config));

    let response = app
        .oneshot(Request::new(Body::empty()))
        .await
        .expect("response");
    let id = response.headers()["x-request-id"]
        .to_str()
        .expect("ASCII ID");
    assert_eq!(id.len(), 32);
    assert!(
        id.bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
}

#[tokio::test(flavor = "current_thread")]
async fn generator_none_falls_back_and_validator_applies_only_to_caller_input() {
    let none_config = ObservabilityConfig::default().with_request_id_generator(|| None);
    let none_app = Router::new()
        .route("/", get(context_handler))
        .layer(ObservabilityLayer::new(none_config));
    let none_response = none_app
        .oneshot(Request::new(Body::empty()))
        .await
        .expect("response");
    let none_id = none_response.headers()["x-request-id"]
        .to_str()
        .expect("request ID text");
    assert_eq!(
        RequestId::parse(none_id).expect("valid fallback").as_str(),
        none_id
    );

    let attempts = Arc::new(AtomicUsize::new(0));
    let generator_attempts = attempts.clone();
    let rejected_config = ObservabilityConfig::default()
        .with_request_id_generator(move || {
            generator_attempts.fetch_add(1, Ordering::SeqCst);
            Some(RequestId::parse("validator-rejected").expect("valid generated ID"))
        })
        .with_request_id_validator(|_| false);
    let rejected_app = Router::new()
        .route("/", get(context_handler))
        .layer(ObservabilityLayer::new(rejected_config));
    let rejected_response = rejected_app
        .oneshot(Request::new(Body::empty()))
        .await
        .expect("response");
    let rejected_id = rejected_response.headers()["x-request-id"]
        .to_str()
        .expect("request ID text");
    assert_eq!(rejected_id, "validator-rejected");
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn custom_validator_may_admit_native_safe_values_beyond_default_grammar() {
    for request_id in ["id:42".to_owned(), "a".repeat(129)] {
        let config = ObservabilityConfig::default().with_request_id_validator(|_| true);
        let capture = Capture::default();
        let _guard = subscriber(&config, capture).set_default();
        let app = Router::new()
            .route("/", get(context_handler))
            .layer(ObservabilityLayer::new(config));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header("x-request-id", request_id.as_str())
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        assert_eq!(
            response.headers()["x-request-id"]
                .to_str()
                .expect("request ID text"),
            request_id
        );
    }
}
