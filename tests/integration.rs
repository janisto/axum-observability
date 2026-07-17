//! End-to-end middleware and formatter contract tests.

use std::{
    collections::{BTreeMap, BTreeSet},
    convert::Infallible,
    future::{Ready, ready},
    io,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    task::{Context, Poll},
    time::{Duration, Instant},
};

#[cfg(feature = "peer-ip")]
use axum::extract::ConnectInfo;
use axum::{
    Extension, Router,
    body::{Body, Bytes, to_bytes},
    http::{HeaderMap, HeaderName, HeaderValue, Request, Response, StatusCode},
    response::IntoResponse,
    routing::get,
};
use axum_observability::{
    FieldConvention, MissingRequestContext, ObservabilityConfig, ObservabilityLayer, OperationId,
    RequestContext, RequestId,
};
use http_body::{Body as HttpBody, Frame};
use serde_json::Value;
#[cfg(feature = "peer-ip")]
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use tower::{Layer, Service, ServiceExt};
use tower_http::{catch_panic::CatchPanicLayer, timeout::TimeoutLayer};
use tracing_subscriber::{
    Layer as SubscriberLayer,
    filter::{LevelFilter, Targets},
    layer::SubscriberExt,
    util::SubscriberInitExt,
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

#[cfg(feature = "peer-ip")]
async fn privacy_record(config: ObservabilityConfig) -> Value {
    let capture = Capture::default();
    let _guard = subscriber(&config, capture.clone()).set_default();
    let app = Router::new()
        .route("/items/{id}", get(|| async { StatusCode::NO_CONTENT }))
        .layer(ObservabilityLayer::new(config));
    let mut request = Request::builder()
        .uri("/items/42?secret=query")
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
    capture.records().remove(0)
}

#[cfg(feature = "peer-ip")]
async fn representative_access_record(convention: FieldConvention) -> Value {
    let config = ObservabilityConfig::default()
        .with_field_convention(convention)
        .with_raw_path(true)
        .with_peer_ip(true)
        .with_user_agent(true)
        .with_access_enricher(|_| {
            BTreeMap::from([("tenant".to_owned(), Value::String("public".to_owned()))])
        });
    let capture = Capture::default();
    let _guard = subscriber(&config, capture.clone()).set_default();
    let app = Router::new()
        .route(
            "/items/{id}",
            get(|| async {
                (
                    Extension(OperationId::from_static("get-item")),
                    StatusCode::OK,
                )
            }),
        )
        .layer(ObservabilityLayer::new(config));
    let mut request = Request::builder()
        .uri("/items/42?secret=query")
        .header("x-request-id", "request-42")
        .header("user-agent", "agent/1")
        .header(
            "traceparent",
            "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01",
        )
        .body(Body::empty())
        .expect("request");
    request.extensions_mut().insert(ConnectInfo(SocketAddr::new(
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        3210,
    )));
    app.oneshot(request).await.expect("response");
    capture.records().remove(0)
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

#[derive(Clone)]
struct FailingCapture {
    state: Arc<Mutex<FailingWriterState>>,
    prefix_len: usize,
}

#[derive(Default)]
struct FailingWriterState {
    calls: usize,
    bytes: Vec<u8>,
}

struct FailingWriter {
    state: Arc<Mutex<FailingWriterState>>,
    prefix_len: usize,
}

impl io::Write for FailingWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let mut state = self.state.lock().expect("failing writer lock");
        state.calls += 1;
        if state.bytes.is_empty() && self.prefix_len > 0 {
            let written = self.prefix_len.min(bytes.len());
            state.bytes.extend_from_slice(&bytes[..written]);
            Ok(written)
        } else {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "secret writer detail",
            ))
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'writer> tracing_subscriber::fmt::MakeWriter<'writer> for FailingCapture {
    type Writer = FailingWriter;

    fn make_writer(&'writer self) -> Self::Writer {
        FailingWriter {
            state: self.state.clone(),
            prefix_len: self.prefix_len,
        }
    }
}

fn subscriber(config: &ObservabilityConfig, capture: Capture) -> impl tracing::Subscriber {
    tracing_subscriber::registry().with(config.json_layer(capture))
}

async fn context_handler(context: RequestContext) -> String {
    format!("{}|{}", context.request_id(), context.correlation_id())
}

#[tokio::test(flavor = "current_thread")]
async fn missing_request_context_has_a_stable_non_sensitive_rejection() {
    fn assert_standard_traits<T: Clone + Copy + std::fmt::Debug + Eq + std::error::Error>() {}
    assert_standard_traits::<MissingRequestContext>();

    let response = Router::new()
        .route("/", get(context_handler))
        .oneshot(Request::new(Body::empty()))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body = to_bytes(response.into_body(), 1_024).await.expect("body");
    assert_eq!(body, "request context unavailable");
}

async fn canonical_identity_handler(context: RequestContext, headers: HeaderMap) -> String {
    let values = headers
        .get_all("x-request-id")
        .iter()
        .map(|value| value.to_str().expect("canonical ID is text"))
        .collect::<Vec<_>>();
    format!("{}|{}|{}", context.request_id(), values.len(), values[0])
}

async fn canonical_logging_handler(context: RequestContext, headers: HeaderMap) -> String {
    tracing::info!("identity observed");
    canonical_identity_handler(context, headers).await
}

async fn custom_identity_handler(context: RequestContext, headers: HeaderMap) -> String {
    format!(
        "{}|{}|{}",
        context.request_id(),
        headers.get_all("x-correlation-id").iter().count(),
        headers.get("x-request-id").is_none()
    )
}

async fn gcp_health_handler() -> &'static str {
    tracing::info!(
        service_name = "example-service",
        service_version = "0.3.0",
        health_status = "ok",
        "health check"
    );
    tracing::debug!(
        dependency = "database",
        dependency_status = "ok",
        check_duration_ms = 3_u64,
        "dependency check"
    );
    "ok"
}

async fn gcp_health_records(filter: LevelFilter) -> (StatusCode, Bytes, Vec<Value>) {
    let config = ObservabilityConfig::default()
        .with_field_convention(FieldConvention::Gcp)
        .with_raw_path(true);
    let capture = Capture::default();
    let layer = config
        .json_layer(capture.clone())
        .with_filter(Targets::new().with_default(filter));
    let _guard = tracing_subscriber::registry().with(layer).set_default();
    let app = Router::new()
        .route("/health", get(gcp_health_handler))
        .layer(ObservabilityLayer::new(config));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .header("x-request-id", "health-example")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    let status = response.status();
    let body = to_bytes(response.into_body(), 1_024).await.expect("body");
    (status, body, capture.records())
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
async fn generator_failure_falls_back_even_when_custom_policy_rejects_everything() {
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
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn generator_none_and_validator_rejection_use_package_owned_ids() {
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
    assert_ne!(rejected_id, "validator-rejected");
    assert!(RequestId::parse(rejected_id).is_ok());
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
}

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
        assert!(record["duration_ms"].is_f64());
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
async fn ambiguous_or_non_text_user_agent_is_omitted() {
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
    app.oneshot(request).await.expect("response");

    let records = capture.records();
    assert_eq!(records.len(), 2);
    assert!(
        records
            .iter()
            .all(|record| record.get("user_agent").is_none())
    );
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

#[tokio::test(flavor = "current_thread")]
async fn custom_header_validator_and_response_header_configuration_are_effective() {
    let config = ObservabilityConfig::default()
        .with_request_id_header(HeaderName::from_static("x-correlation-id"))
        .with_request_id_validator(|value| value.as_str().starts_with("custom-"))
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
        .insert(OperationId::from_static("list-items"));
    let response = app.oneshot(request).await.expect("response");
    to_bytes(response.into_body(), 1_024).await.expect("body");

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
