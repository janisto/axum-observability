use super::*;

#[tokio::test(flavor = "current_thread")]
async fn emits_once_at_body_eof_with_final_status_and_non_negative_duration() {
    let origin = Instant::now();
    let calls = Arc::new(AtomicUsize::new(0));
    let clock_calls = calls.clone();
    let config = ObservabilityConfig::default().with_clock(move || {
        if clock_calls.fetch_add(1, Ordering::SeqCst) == 0 {
            origin + Duration::from_secs(2)
        } else {
            origin + Duration::from_secs(1)
        }
    });
    let capture = Capture::default();
    let _guard = subscriber(&config, capture.clone()).set_default();
    let app = Router::new()
        .route(
            "/",
            get(|| async { (StatusCode::IM_A_TEAPOT, "response body") }),
        )
        .layer(ObservabilityLayer::new(config));
    let response = app
        .oneshot(Request::new(Body::empty()))
        .await
        .expect("response");
    assert!(capture.records().is_empty());
    to_bytes(response.into_body(), 1_024).await.expect("body");

    let records = capture.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["status"], 418);
    assert_eq!(records[0]["level"], "WARN");
    assert_eq!(records[0]["duration_ms"], 0.0);
    assert!(records[0].get("terminal_reason").is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn already_ended_body_finishes_before_exposing_eof() {
    let config = ObservabilityConfig::default();
    let capture = Capture::default();
    let _guard = subscriber(&config, capture.clone()).set_default();
    let app = Router::new()
        .route("/", get(|| async { Body::empty() }))
        .layer(ObservabilityLayer::new(config));

    let response = app
        .oneshot(Request::new(Body::empty()))
        .await
        .expect("response");

    assert!(response.body().is_end_stream());
    let records = capture.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["status"], 200);
    assert!(records[0].get("terminal_reason").is_none());

    drop(response);
    assert_eq!(capture.records().len(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn dropping_an_unread_response_emits_one_abnormal_terminal_record() {
    let config = ObservabilityConfig::default().with_status_level_mapper(|_| tracing::Level::DEBUG);
    let capture = Capture::default();
    let _guard = subscriber(&config, capture.clone()).set_default();
    let app = Router::new()
        .route("/", get(|| async { "unread" }))
        .layer(ObservabilityLayer::new(config));
    let response = app
        .oneshot(Request::new(Body::empty()))
        .await
        .expect("response");
    assert!(capture.records().is_empty());
    drop(response);

    let records = capture.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["status"], 200);
    assert_eq!(records[0]["level"], "DEBUG");
    assert_eq!(records[0]["terminal_reason"], "response_dropped");
}

struct FailingBody(bool);

impl HttpBody for FailingBody {
    type Data = Bytes;
    type Error = io::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        _context: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        if self.0 {
            Poll::Ready(None)
        } else {
            self.0 = true;
            Poll::Ready(Some(Err(io::Error::other("stream failed"))))
        }
    }
}

struct FinalFrameBody(Option<Bytes>);

impl HttpBody for FinalFrameBody {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        _context: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        Poll::Ready(self.0.take().map(|bytes| Ok(Frame::data(bytes))))
    }

    fn is_end_stream(&self) -> bool {
        self.0.is_none()
    }
}

#[tokio::test(flavor = "current_thread")]
async fn final_frame_completes_before_the_consumer_polls_again_or_drops() {
    let config = ObservabilityConfig::default();
    let capture = Capture::default();
    let _guard = subscriber(&config, capture.clone()).set_default();
    let app = Router::new()
        .route(
            "/",
            get(|| async {
                Response::new(Body::new(FinalFrameBody(Some(Bytes::from_static(
                    b"final",
                )))))
            }),
        )
        .layer(ObservabilityLayer::new(config));
    let response = app
        .oneshot(Request::new(Body::empty()))
        .await
        .expect("response");
    assert!(capture.records().is_empty());

    let mut body = response.into_body();
    let frame = std::future::poll_fn(|context| Pin::new(&mut body).poll_frame(context))
        .await
        .expect("final frame")
        .expect("successful final frame");
    assert_eq!(frame.into_data().expect("data frame"), "final");

    let records = capture.records();
    assert_eq!(records.len(), 1);
    assert!(records[0].get("terminal_reason").is_none());
    drop(body);
    assert_eq!(capture.records().len(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn body_error_emits_once_with_controlled_error_information() {
    let config = ObservabilityConfig::default().with_status_level_mapper(|_| tracing::Level::TRACE);
    let capture = Capture::default();
    let filtered = config
        .json_layer(capture.clone())
        .with_filter(Targets::new().with_default(LevelFilter::WARN));
    let _guard = tracing_subscriber::registry().with(filtered).set_default();
    let app = Router::new()
        .route(
            "/",
            get(|| async { Response::new(Body::new(FailingBody(false))) }),
        )
        .layer(ObservabilityLayer::new(config));
    let response = app
        .oneshot(Request::new(Body::empty()))
        .await
        .expect("response");
    assert!(to_bytes(response.into_body(), 1_024).await.is_err());

    let records = capture.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["status"], 200);
    assert_eq!(records[0]["level"], "ERROR");
    assert_eq!(records[0]["terminal_reason"], "body_error");
    assert_eq!(records[0]["error"], "response body failed");
}

#[tokio::test(flavor = "current_thread")]
async fn service_error_is_returned_and_emitted_once_at_error() {
    let config = ObservabilityConfig::default().with_status_level_mapper(|_| tracing::Level::TRACE);
    let capture = Capture::default();
    let filtered = config
        .json_layer(capture.clone())
        .with_filter(Targets::new().with_default(LevelFilter::WARN));
    let _guard = tracing_subscriber::registry().with(filtered).set_default();
    let service =
        ObservabilityLayer::new(config).layer(tower::service_fn(|_request: Request<Body>| async {
            Err::<Response<Body>, _>(io::Error::other("original service error"))
        }));

    let error = service
        .oneshot(Request::new(Body::empty()))
        .await
        .expect_err("service must fail");
    assert_eq!(error.to_string(), "original service error");

    let records = capture.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["level"], "ERROR");
    assert_eq!(records[0]["terminal_reason"], "service_error");
    assert_eq!(records[0]["error"], "downstream service failed");
    assert!(records[0].get("status").is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn completed_statuses_use_the_configured_mapper_once() {
    let config = ObservabilityConfig::default().with_status_level_mapper(|status| match status {
        StatusCode::OK => tracing::Level::DEBUG,
        StatusCode::BAD_REQUEST => tracing::Level::WARN,
        StatusCode::INTERNAL_SERVER_ERROR => tracing::Level::ERROR,
        _ => tracing::Level::TRACE,
    });
    let capture = Capture::default();
    let _guard = subscriber(&config, capture.clone()).set_default();
    let app = Router::new()
        .route("/ok", get(|| async { StatusCode::OK }))
        .route("/bad", get(|| async { StatusCode::BAD_REQUEST }))
        .route(
            "/error",
            get(|| async { StatusCode::INTERNAL_SERVER_ERROR }),
        )
        .layer(ObservabilityLayer::new(config));

    for path in ["/ok", "/bad", "/error"] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("response");
        to_bytes(response.into_body(), 1_024).await.expect("body");
    }

    let records = capture.records();
    assert_eq!(records.len(), 3);
    assert_eq!(records[0]["level"], "DEBUG");
    assert_eq!(records[1]["level"], "WARN");
    assert_eq!(records[2]["level"], "ERROR");
    assert!(
        records
            .iter()
            .all(|record| record.get("terminal_reason").is_none())
    );
}

#[tokio::test(flavor = "current_thread")]
async fn initial_clock_panic_is_contained_and_request_completes() {
    let calls = Arc::new(AtomicUsize::new(0));
    let clock_calls = calls.clone();
    let config = ObservabilityConfig::default().with_clock(move || {
        assert_ne!(
            clock_calls.fetch_add(1, Ordering::SeqCst),
            0,
            "initial clock failure"
        );
        Instant::now()
    });
    let capture = Capture::default();
    let _guard = subscriber(&config, capture.clone()).set_default();
    let app = Router::new()
        .route("/", get(|| async { StatusCode::NO_CONTENT }))
        .layer(ObservabilityLayer::new(config));

    let response = app
        .oneshot(Request::new(Body::empty()))
        .await
        .expect("clock panic must not replace the response");
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    let records = capture.records();
    assert_eq!(records.len(), 1);
    assert!(records[0]["duration_ms"].as_f64().expect("duration") >= 0.0);
}

#[tokio::test(flavor = "current_thread")]
async fn finish_clock_panic_falls_back_to_zero_duration() {
    let started = Instant::now();
    let calls = Arc::new(AtomicUsize::new(0));
    let clock_calls = calls.clone();
    let config = ObservabilityConfig::default().with_clock(move || {
        if clock_calls.fetch_add(1, Ordering::SeqCst) == 0 {
            started
        } else {
            panic!("finish clock failure");
        }
    });
    let capture = Capture::default();
    let _guard = subscriber(&config, capture.clone()).set_default();
    let app = Router::new()
        .route("/", get(|| async { StatusCode::NO_CONTENT }))
        .layer(ObservabilityLayer::new(config));

    let response = app
        .oneshot(Request::new(Body::empty()))
        .await
        .expect("clock panic must not replace the response");
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    assert_eq!(capture.records()[0]["duration_ms"], 0.0);
}

#[tokio::test(flavor = "current_thread")]
async fn mapper_panic_uses_the_default_status_level() {
    let config =
        ObservabilityConfig::default().with_status_level_mapper(|_| panic!("mapper failure"));
    let capture = Capture::default();
    let _guard = subscriber(&config, capture.clone()).set_default();
    let app = Router::new()
        .route("/", get(|| async { StatusCode::IM_A_TEAPOT }))
        .layer(ObservabilityLayer::new(config));

    let response = app
        .oneshot(Request::new(Body::empty()))
        .await
        .expect("mapper panic must not replace the response");
    assert_eq!(response.status(), StatusCode::IM_A_TEAPOT);
    assert_eq!(capture.records()[0]["level"], "WARN");
}

#[tokio::test(flavor = "current_thread")]
async fn enricher_panic_uses_an_empty_enrichment() {
    let config =
        ObservabilityConfig::default().with_access_enricher(|_| panic!("enricher failure"));
    let capture = Capture::default();
    let _guard = subscriber(&config, capture.clone()).set_default();
    let app = Router::new()
        .route("/", get(|| async { StatusCode::NO_CONTENT }))
        .layer(ObservabilityLayer::new(config));

    let response = app
        .oneshot(Request::new(Body::empty()))
        .await
        .expect("enricher panic must not replace the response");
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let record = &capture.records()[0];
    assert_eq!(record["status"], 204);
    assert_eq!(record["message"], "request completed");
}

#[tokio::test(flavor = "current_thread")]
async fn outer_observability_records_recovered_panics_and_timeouts_with_final_status() {
    async fn panic_handler() -> StatusCode {
        panic!("controlled test panic")
    }

    let config = ObservabilityConfig::default();
    let capture = Capture::default();
    let _guard = subscriber(&config, capture.clone()).set_default();
    let app = Router::new()
        .route("/panic", get(panic_handler))
        .route(
            "/timeout",
            get(|| async {
                tokio::time::sleep(Duration::from_millis(20)).await;
                StatusCode::OK
            }),
        )
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            Duration::from_millis(1),
        ))
        .layer(CatchPanicLayer::new())
        .layer(ObservabilityLayer::new(config));

    for (path, expected) in [
        ("/panic", StatusCode::INTERNAL_SERVER_ERROR),
        ("/timeout", StatusCode::REQUEST_TIMEOUT),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(path)
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("recovered response");
        assert_eq!(response.status(), expected);
        to_bytes(response.into_body(), 1_024).await.expect("body");
    }

    let access = capture
        .records()
        .into_iter()
        .filter(|record| record["message"] == "request completed")
        .collect::<Vec<_>>();
    assert_eq!(access.len(), 2);
    assert_eq!(access[0]["status"], 500);
    assert_eq!(access[0]["level"], "ERROR");
    assert_eq!(access[1]["status"], 408);
    assert_eq!(access[1]["level"], "WARN");
    assert!(
        access
            .iter()
            .all(|record| record.get("terminal_reason").is_none())
    );
}

#[tokio::test(flavor = "current_thread")]
async fn dropping_an_unpolled_service_future_still_emits_once() {
    let config = ObservabilityConfig::default().with_status_level_mapper(|_| tracing::Level::ERROR);
    let capture = Capture::default();
    let _guard = subscriber(&config, capture.clone()).set_default();
    let mut service =
        ObservabilityLayer::new(config).layer(tower::service_fn(|_request: Request<Body>| async {
            Ok::<_, std::convert::Infallible>(Response::new(Body::empty()))
        }));

    let future = service.call(Request::new(Body::empty()));
    drop(future);

    let records = capture.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["level"], "WARN");
    assert_eq!(records[0]["terminal_reason"], "response_dropped");
    assert!(records[0].get("status").is_none());
}
