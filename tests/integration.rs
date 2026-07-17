#![allow(missing_docs)]

use std::{
    collections::BTreeMap,
    convert::Infallible,
    future::{Ready, ready},
    io,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll},
    time::{Duration, Instant},
};

use axum::{
    Extension, Router,
    body::{Body, Bytes},
    extract::ConnectInfo,
    http::{HeaderValue, Request, Response, StatusCode},
    response::IntoResponse,
    routing::get,
};
use axum_observability::{
    ObservabilityConfig, ObservabilityLayer, OperationId, Preset, RequestContext,
};
use http_body::{Body as HttpBody, Frame};
use http_body_util::BodyExt;
use serde_json::Value;
use tower::{Layer, Service, ServiceExt};
use tower_http::{catch_panic::CatchPanicLayer, timeout::TimeoutLayer};
use tracing_subscriber::{
    Layer as SubscriberLayer, filter::EnvFilter, layer::SubscriberExt, util::SubscriberInitExt,
};

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
        let bytes = self.0.lock().expect("capture lock").clone();
        String::from_utf8(bytes)
            .expect("JSON is UTF-8")
            .lines()
            .map(|line| serde_json::from_str(line).expect("line is JSON"))
            .collect()
    }
}

#[derive(Clone, Default)]
struct CountingCapture {
    capture: Capture,
    writes: Arc<AtomicUsize>,
}

struct CountingWriter {
    capture: CaptureWriter,
    writes: Arc<AtomicUsize>,
}

impl io::Write for CountingWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.writes.fetch_add(1, Ordering::SeqCst);
        self.capture.write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.capture.flush()
    }
}

impl<'writer> tracing_subscriber::fmt::MakeWriter<'writer> for CountingCapture {
    type Writer = CountingWriter;

    fn make_writer(&'writer self) -> Self::Writer {
        CountingWriter {
            capture: CaptureWriter(self.capture.0.clone()),
            writes: self.writes.clone(),
        }
    }
}

fn subscriber(config: &ObservabilityConfig, capture: Capture) -> impl tracing::Subscriber {
    tracing_subscriber::registry().with(config.json_layer(capture))
}

async fn context_handler(context: RequestContext) -> String {
    format!("{}|{}", context.request_id(), context.correlation_id())
}

#[derive(Clone)]
struct SynchronousLoggingService;

impl Service<Request<Body>> for SynchronousLoggingService {
    type Response = Response<Body>;
    type Error = Infallible;
    type Future = Ready<Result<Self::Response, Self::Error>>;

    fn poll_ready(&mut self, _context: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, _request: Request<Body>) -> Self::Future {
        tracing::warn!("synchronous service call");
        ready(Ok(Response::new(Body::empty())))
    }
}

#[tokio::test(flavor = "current_thread")]
async fn accepts_one_valid_request_id_and_returns_it_on_the_response() {
    let config = ObservabilityConfig::default();
    let capture = Capture::default();
    let _guard = subscriber(&config, capture.clone()).set_default();
    let app = Router::new()
        .route("/context", get(context_handler))
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

    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    assert_eq!(body, "safe_ID~1|safe_ID~1");
    let records = capture.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["path"], "/context");
    assert_eq!(records[0]["path_template"], "/context");
    assert!(records[0].to_string().find("secret").is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn replaces_duplicate_ids_and_retries_a_bad_custom_generator_once() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let generator_attempts = attempts.clone();
    let config = ObservabilityConfig::default().with_request_id_generator(move || {
        let attempt = generator_attempts.fetch_add(1, Ordering::SeqCst);
        Some(
            if attempt == 0 {
                "not valid"
            } else {
                "retry-ok"
            }
            .to_owned(),
        )
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
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    assert_eq!(response.headers()["x-request-id"], "retry-ok");
}

#[tokio::test(flavor = "current_thread")]
async fn generator_failure_falls_back_even_when_custom_policy_rejects_everything() {
    let config = ObservabilityConfig::default()
        .with_request_id_generator(|| panic!("generator failure"))
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
}

#[tokio::test(flavor = "current_thread")]
async fn valid_trace_context_correlates_application_and_access_events_without_spoofing() {
    async fn handler(context: RequestContext) -> impl IntoResponse {
        tracing::info!(request_id = "spoofed", answer = 42_u64, "application event");
        assert_eq!(context.correlation_id(), "4bf92f3577b34da6a3ce929d0e0e4736");
        StatusCode::NO_CONTENT
    }

    let config = ObservabilityConfig::default().with_preset(Preset::Gcp);
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
    response.into_body().collect().await.expect("body");
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
    assert_eq!(records[1]["httpRequest"]["requestUrl"], "/items/42");
    assert!(records[1]["httpRequest"]["status"].is_u64());
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
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    assert_eq!(body, "request-only|request-only");
}

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
    response.into_body().collect().await.expect("body");

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
    let config = ObservabilityConfig::default();
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
    let frame = body
        .frame()
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
    let config = ObservabilityConfig::default();
    let capture = Capture::default();
    let _guard = subscriber(&config, capture.clone()).set_default();
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
    assert!(response.into_body().collect().await.is_err());

    let records = capture.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["terminal_reason"], "body_error");
    assert_eq!(records[0]["error"], "response body failed");
}

#[tokio::test(flavor = "current_thread")]
async fn cloud_presets_emit_exact_provider_trace_shapes() {
    for (preset, key, expected) in [
        (
            Preset::Aws,
            "xray_trace_id",
            "1-4bf92f35-77b34da6a3ce929d0e0e4736",
        ),
        (
            Preset::Azure,
            "operation_Id",
            "4bf92f3577b34da6a3ce929d0e0e4736",
        ),
    ] {
        let config = ObservabilityConfig::default().with_preset(preset);
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
        app.oneshot(request)
            .await
            .expect("response")
            .into_body()
            .collect()
            .await
            .expect("body");
        let records = capture.records();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0][key], expected);
        if preset == Preset::Azure {
            assert_eq!(records[0]["operation_ParentId"], "00f067aa0ba902b7");
        }
    }
}

#[test]
fn invalid_request_span_trace_ids_do_not_emit_aws_metadata() {
    let config = ObservabilityConfig::default().with_preset(Preset::Aws);
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
    let config = ObservabilityConfig::default().with_preset(Preset::Gcp);
    let capture = Capture::default();
    let filtered = config
        .json_layer(capture.clone())
        .with_filter(EnvFilter::new("warn"));
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
    let config = ObservabilityConfig::default().with_preset(Preset::Gcp);
    let capture = Capture::default();
    let filtered = config
        .json_layer(capture.clone())
        .with_filter(EnvFilter::new("warn,axum_observability::request=info"));
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
    let config = ObservabilityConfig::default().with_preset(Preset::Gcp);
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
async fn peer_and_unambiguous_user_agent_are_recorded_without_forwarded_headers() {
    let config = ObservabilityConfig::default();
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
    app.oneshot(request)
        .await
        .expect("response")
        .into_body()
        .collect()
        .await
        .expect("body");
    let records = capture.records();
    assert_eq!(records[0]["remote_ip"], "127.0.0.1");
    assert_eq!(records[0]["user_agent"], "agent/1");
    assert!(records[0].to_string().find("203.0.113.7").is_none());
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
        response.into_body().collect().await.expect("body");
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
    let config = ObservabilityConfig::default();
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
    assert_eq!(records[0]["terminal_reason"], "response_dropped");
    assert!(records[0].get("status").is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn custom_header_validator_and_response_header_configuration_are_effective() {
    let config = ObservabilityConfig::default()
        .with_request_id_header("x-correlation-id")
        .expect("valid custom header")
        .with_request_id_validator(|value| value.starts_with("custom-"))
        .with_request_id_generator(|| Some("custom-generated".to_owned()));
    assert!(
        ObservabilityConfig::default()
            .with_request_id_header("not a header")
            .is_err()
    );
    let capture = Capture::default();
    let _guard = subscriber(&config, capture).set_default();
    let app = Router::new()
        .route("/", get(context_handler))
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
    let body = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    assert_eq!(body, "custom-generated|custom-generated");

    let disabled = ObservabilityConfig::default().with_response_header(false);
    let capture = Capture::default();
    let _guard = subscriber(&disabled, capture).set_default();
    let app = Router::new()
        .route("/", get(|| async { StatusCode::NO_CONTENT }))
        .layer(ObservabilityLayer::new(disabled));
    let response = app
        .oneshot(Request::new(Body::empty()))
        .await
        .expect("response");
    assert!(response.headers().get("x-request-id").is_none());
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
        .insert(OperationId::new("list-items"));
    app.oneshot(request)
        .await
        .expect("response")
        .into_body()
        .collect()
        .await
        .expect("body");

    let records = capture.records();
    let record = &records[0];
    assert_eq!(record["level"], "ERROR");
    assert_eq!(record["duration_ms"], 1_500.0);
    assert_eq!(record["tenant"], "public");
    assert_eq!(record["status"], 200);
    assert_eq!(record["target"], "axum_observability::access");
    assert_eq!(record["request_id"], "real-request");
    assert_eq!(record["operation_id"], "list-items");
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
                    Extension(OperationId::new("route-list-items")),
                    StatusCode::NO_CONTENT,
                )
            }),
        )
        .layer(ObservabilityLayer::new(config));
    let mut request = Request::new(Body::empty());
    request
        .extensions_mut()
        .insert(OperationId::new("preseeded-operation"));

    app.oneshot(request).await.expect("response");

    let records = capture.records();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0]["operation_id"], "route-list-items");
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

#[test]
fn gcp_uses_canonical_cloud_logging_severity_names() {
    let config = ObservabilityConfig::default().with_preset(Preset::Gcp);
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
async fn observed_body_delegates_stream_state_and_size_hint() {
    let config = ObservabilityConfig::default();
    let capture = Capture::default();
    let _guard = subscriber(&config, capture).set_default();
    let app = Router::new()
        .route("/full", get(|| async { "hello" }))
        .route("/empty", get(|| async { Body::empty() }))
        .layer(ObservabilityLayer::new(config));

    let full = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/full")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert!(!full.body().is_end_stream());
    assert_eq!(full.body().size_hint().exact(), Some(5));

    let empty = app
        .oneshot(
            Request::builder()
                .uri("/empty")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert!(empty.body().is_end_stream());
    assert_eq!(empty.body().size_hint().exact(), Some(0));
}

#[tokio::test(flavor = "current_thread")]
async fn poll_ready_errors_are_delegated_to_the_inner_service() {
    #[derive(Clone)]
    struct ReadinessFailure;

    impl Service<Request<Body>> for ReadinessFailure {
        type Response = Response<Body>;
        type Error = io::Error;
        type Future = std::future::Ready<Result<Self::Response, Self::Error>>;

        fn poll_ready(&mut self, _context: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Err(io::Error::other("not ready")))
        }

        fn call(&mut self, _request: Request<Body>) -> Self::Future {
            unreachable!("call must not run when readiness fails")
        }
    }

    let mut service = ObservabilityLayer::default().layer(ReadinessFailure);
    let error = std::future::poll_fn(|context| service.poll_ready(context))
        .await
        .expect_err("readiness failure");
    assert_eq!(error.to_string(), "not ready");
}
