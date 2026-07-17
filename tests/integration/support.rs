use super::*;

#[derive(Clone, Default)]
pub(super) struct Capture(Arc<Mutex<Vec<u8>>>);

pub(super) struct CaptureWriter(Arc<Mutex<Vec<u8>>>);

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
    pub(super) fn records(&self) -> Vec<Value> {
        let bytes = self.0.lock().expect("capture lock").clone();
        String::from_utf8(bytes)
            .expect("JSON is UTF-8")
            .lines()
            .map(|line| serde_json::from_str(line).expect("line is JSON"))
            .collect()
    }
}

#[cfg(feature = "peer-ip")]
pub(super) async fn privacy_record(config: ObservabilityConfig) -> Value {
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
pub(super) async fn representative_access_record(convention: FieldConvention) -> Value {
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
pub(super) struct CountingCapture {
    pub(super) capture: Capture,
    pub(super) writes: Arc<AtomicUsize>,
}

pub(super) struct CountingWriter {
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
pub(super) struct FailingCapture {
    pub(super) state: Arc<Mutex<FailingWriterState>>,
    pub(super) prefix_len: usize,
}

#[derive(Default)]
pub(super) struct FailingWriterState {
    pub(super) calls: usize,
    pub(super) bytes: Vec<u8>,
}

pub(super) struct FailingWriter {
    pub(super) state: Arc<Mutex<FailingWriterState>>,
    pub(super) prefix_len: usize,
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

pub(super) fn subscriber(
    config: &ObservabilityConfig,
    capture: Capture,
) -> impl tracing::Subscriber {
    tracing_subscriber::registry().with(config.json_layer(capture))
}

pub(super) async fn context_handler(context: RequestContext) -> String {
    format!("{}|{}", context.request_id(), context.correlation_id())
}

pub(super) async fn canonical_identity_handler(
    context: RequestContext,
    headers: HeaderMap,
) -> String {
    let values = headers
        .get_all("x-request-id")
        .iter()
        .map(|value| value.to_str().expect("canonical ID is text"))
        .collect::<Vec<_>>();
    format!("{}|{}|{}", context.request_id(), values.len(), values[0])
}

pub(super) async fn canonical_logging_handler(
    context: RequestContext,
    headers: HeaderMap,
) -> String {
    tracing::info!("identity observed");
    canonical_identity_handler(context, headers).await
}

pub(super) async fn custom_identity_handler(context: RequestContext, headers: HeaderMap) -> String {
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

pub(super) async fn gcp_health_records(filter: LevelFilter) -> (StatusCode, Bytes, Vec<Value>) {
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
pub(super) struct SynchronousLoggingService;

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
