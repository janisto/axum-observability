use std::{
    collections::BTreeMap,
    fmt,
    future::Future,
    panic::{AssertUnwindSafe, catch_unwind},
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Instant,
};

#[cfg(feature = "peer-ip")]
use axum::extract::ConnectInfo;
use axum::{
    body::{Body, Bytes},
    extract::MatchedPath,
    http::{HeaderMap, HeaderName, HeaderValue, Request, Response, StatusCode, header::USER_AGENT},
};
use http_body::{Body as HttpBody, Frame, SizeHint};
use pin_project_lite::pin_project;
use serde::Serialize;
use serde_json::Value;
#[cfg(feature = "peer-ip")]
use std::net::SocketAddr;
use tower_layer::Layer;
use tower_service::Service;
use tracing::{Instrument, Level, Span};
use uuid::Uuid;

use crate::{
    FieldConvention, JsonLayer, OperationId, RequestContext, RequestId, TraceContext,
    trace_context::{parse_traceparent, parse_tracestate},
};

type Generator = Arc<dyn Fn() -> Option<RequestId> + Send + Sync>;
type Validator = Arc<dyn Fn(&RequestId) -> bool + Send + Sync>;
type LevelMapper = Arc<dyn Fn(StatusCode) -> Level + Send + Sync>;
type Clock = Arc<dyn Fn() -> Instant + Send + Sync>;
type Enricher = Arc<dyn Fn(&RequestContext) -> BTreeMap<String, Value> + Send + Sync>;

/// Configuration for [`ObservabilityLayer`].
#[derive(Clone)]
#[must_use]
#[allow(
    clippy::struct_excessive_bools,
    reason = "independent opt-in capture and response policies are explicit configuration"
)]
pub struct ObservabilityConfig {
    pub(crate) field_convention: FieldConvention,
    request_id_header: HeaderName,
    response_header: bool,
    raw_path: bool,
    #[cfg(feature = "peer-ip")]
    peer_ip: bool,
    user_agent: bool,
    generator: Generator,
    validator: Validator,
    level_mapper: LevelMapper,
    clock: Clock,
    enricher: Enricher,
}

impl fmt::Debug for ObservabilityConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut debug = formatter.debug_struct("ObservabilityConfig");
        debug
            .field("field_convention", &self.field_convention)
            .field("request_id_header", &self.request_id_header)
            .field("response_header", &self.response_header)
            .field("raw_path", &self.raw_path);
        #[cfg(feature = "peer-ip")]
        debug.field("peer_ip", &self.peer_ip);
        debug
            .field("user_agent", &self.user_agent)
            .finish_non_exhaustive()
    }
}

impl Default for ObservabilityConfig {
    fn default() -> Self {
        Self {
            field_convention: FieldConvention::Generic,
            request_id_header: HeaderName::from_static("x-request-id"),
            response_header: true,
            raw_path: false,
            #[cfg(feature = "peer-ip")]
            peer_ip: false,
            user_agent: false,
            generator: Arc::new(|| Some(random_request_id())),
            validator: Arc::new(|_| true),
            level_mapper: Arc::new(default_level),
            clock: Arc::new(Instant::now),
            enricher: Arc::new(|_| BTreeMap::new()),
        }
    }
}

impl ObservabilityConfig {
    /// Selects the provider field convention.
    #[must_use = "configuration builders return a new value"]
    pub fn with_field_convention(mut self, convention: FieldConvention) -> Self {
        self.field_convention = convention;
        self
    }

    /// Sets the request and response correlation header name.
    ///
    /// Use [`HeaderName::from_static`] for a known lowercase name, or
    /// [`HeaderName::try_from`] when configuration supplies the value:
    ///
    /// ```
    /// use axum::http::HeaderName;
    /// use axum_observability::ObservabilityConfig;
    ///
    /// let static_config = ObservabilityConfig::default()
    ///     .with_request_id_header(HeaderName::from_static("x-correlation-id"));
    /// let dynamic_name = HeaderName::try_from("x-runtime-correlation-id")?;
    /// let dynamic_config = ObservabilityConfig::default()
    ///     .with_request_id_header(dynamic_name);
    /// # let _ = (static_config, dynamic_config);
    /// # Ok::<(), axum::http::header::InvalidHeaderName>(())
    /// ```
    #[must_use = "configuration builders return a new value"]
    pub fn with_request_id_header(mut self, name: HeaderName) -> Self {
        self.request_id_header = name;
        self
    }

    /// Enables or disables adding the request ID response header.
    #[must_use = "configuration builders return a new value"]
    pub fn with_response_header(mut self, enabled: bool) -> Self {
        self.response_header = enabled;
        self
    }

    /// Enables or disables query-free raw path capture.
    ///
    /// Enabling this can record identifying data and changes the application's
    /// privacy posture. Query strings are never captured.
    #[must_use = "configuration builders return a new value"]
    pub fn with_raw_path(mut self, enabled: bool) -> Self {
        self.raw_path = enabled;
        self
    }

    /// Enables or disables capture of Axum's trusted socket peer extension.
    ///
    /// Enabling this can record identifying data and changes the application's
    /// privacy posture. Forwarding headers are never inspected.
    #[cfg(feature = "peer-ip")]
    #[must_use = "configuration builders return a new value"]
    pub fn with_peer_ip(mut self, enabled: bool) -> Self {
        self.peer_ip = enabled;
        self
    }

    /// Enables or disables capture of one unambiguous text User-Agent value.
    ///
    /// Enabling this can record identifying data and changes the application's
    /// privacy posture.
    #[must_use = "configuration builders return a new value"]
    pub fn with_user_agent(mut self, enabled: bool) -> Self {
        self.user_agent = enabled;
        self
    }

    /// Sets a fallible request ID generator. It is invoked once per replacement
    /// request before the crate falls back to a package-owned random identifier.
    #[must_use = "configuration builders return a new value"]
    pub fn with_request_id_generator(
        mut self,
        generator: impl Fn() -> Option<RequestId> + Send + Sync + 'static,
    ) -> Self {
        self.generator = Arc::new(generator);
        self
    }

    /// Adds a request ID validator that may narrow, but cannot weaken, the
    /// baseline URI-unreserved policy.
    #[must_use = "configuration builders return a new value"]
    pub fn with_request_id_validator(
        mut self,
        validator: impl Fn(&RequestId) -> bool + Send + Sync + 'static,
    ) -> Self {
        self.validator = Arc::new(validator);
        self
    }

    /// Sets the mapping from final response status to access-event level.
    #[must_use = "configuration builders return a new value"]
    pub fn with_status_level_mapper(
        mut self,
        mapper: impl Fn(StatusCode) -> Level + Send + Sync + 'static,
    ) -> Self {
        self.level_mapper = Arc::new(mapper);
        self
    }

    /// Sets a monotonic clock seam, primarily for deterministic testing.
    ///
    /// Clock panics are contained when the application uses Rust's default
    /// `panic = "unwind"` behavior. Rust code cannot recover from
    /// `panic = "abort"`.
    #[must_use = "configuration builders return a new value"]
    pub fn with_clock(mut self, clock: impl Fn() -> Instant + Send + Sync + 'static) -> Self {
        self.clock = Arc::new(clock);
        self
    }

    /// Adds controlled fields to terminal access records. Reserved package
    /// fields cannot be overwritten.
    #[must_use = "configuration builders return a new value"]
    pub fn with_access_enricher(
        mut self,
        enricher: impl Fn(&RequestContext) -> BTreeMap<String, Value> + Send + Sync + 'static,
    ) -> Self {
        self.enricher = Arc::new(enricher);
        self
    }

    /// Creates a composable JSON layer using this configuration's field convention.
    #[must_use = "configuration builders return a new value"]
    pub fn json_layer<W>(&self, writer: W) -> JsonLayer<W> {
        JsonLayer::new(writer, self.field_convention)
    }

    fn accepts_request_id(&self, value: &RequestId) -> bool {
        catch_unwind(AssertUnwindSafe(|| (self.validator)(value))).unwrap_or(false)
    }

    fn generate_request_id(&self) -> RequestId {
        let generated = catch_unwind(AssertUnwindSafe(|| (self.generator)()))
            .ok()
            .flatten();
        if let Some(value) = generated.filter(|value| self.accepts_request_id(value)) {
            return value;
        }

        random_request_id()
    }
}

/// Cloneable Tower layer that installs correlation and terminal access logs.
#[derive(Clone, Debug)]
#[must_use]
pub struct ObservabilityLayer {
    config: ObservabilityConfig,
}

impl ObservabilityLayer {
    /// Creates a layer from an explicit configuration.
    #[must_use = "configuration builders return a new value"]
    pub const fn new(config: ObservabilityConfig) -> Self {
        Self { config }
    }
}

impl Default for ObservabilityLayer {
    fn default() -> Self {
        Self::new(ObservabilityConfig::default())
    }
}

impl<S> Layer<S> for ObservabilityLayer {
    type Service = ObservabilityService<S>;

    fn layer(&self, inner: S) -> Self::Service {
        ObservabilityService {
            inner,
            config: self.config.clone(),
        }
    }
}

/// Service produced by [`ObservabilityLayer`].
#[derive(Clone, Debug)]
pub struct ObservabilityService<S> {
    inner: S,
    config: ObservabilityConfig,
}

impl<S> Service<Request<Body>> for ObservabilityService<S>
where
    S: Service<Request<Body>, Response = Response<Body>> + Send + 'static,
    S::Future: Send + 'static,
    S::Error: Send + 'static,
{
    type Response = Response<Body>;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send>>;

    fn poll_ready(&mut self, context: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(context)
    }

    fn call(&mut self, mut request: Request<Body>) -> Self::Future {
        let request_id = select_request_id(request.headers_mut(), &self.config);
        let trace_context = select_trace_context(request.headers());
        let request_context = RequestContext::new(request_id, trace_context);
        let metadata = RequestMetadata::from_request(&request, &self.config);
        let span = request_span(&request_context);
        let started = catch_unwind(AssertUnwindSafe(|| (self.config.clock)()))
            .unwrap_or_else(|_| Instant::now());
        let enrichment = catch_unwind(AssertUnwindSafe(|| {
            (self.config.enricher)(&request_context)
        }))
        .unwrap_or_default();

        request.extensions_mut().insert(request_context.clone());
        let future = {
            let _entered = span.enter();
            self.inner.call(request)
        };
        let config = self.config.clone();
        let guard_span = span.clone();
        let guard = TerminalGuard::new(
            metadata,
            request_context,
            started,
            config.clone(),
            guard_span,
            enrichment,
        );

        Box::pin(
            async move {
                let mut guard = guard;
                match future.await {
                    Ok(response) => {
                        let (mut parts, body) = response.into_parts();
                        guard.set_status(parts.status);
                        if let Some(operation_id) = parts.extensions.get::<OperationId>() {
                            guard.set_operation_id(operation_id);
                        }
                        if config.response_header {
                            let value = HeaderValue::from_str(guard.request_id().as_str())
                                .expect("validated request ID is always a header value");
                            parts
                                .headers
                                .insert(config.request_id_header.clone(), value);
                        }
                        Ok(Response::from_parts(
                            parts,
                            Body::new(ObservedBody::new(body, guard)),
                        ))
                    }
                    Err(error) => {
                        guard.finish(TerminalOutcome::ServiceError);
                        Err(error)
                    }
                }
            }
            .instrument(span),
        )
    }
}

fn select_request_id(headers: &mut HeaderMap, config: &ObservabilityConfig) -> RequestId {
    let mut values = headers.get_all(&config.request_id_header).iter();
    let first = values
        .next()
        .and_then(|value| value.to_str().ok())
        .and_then(|value| RequestId::parse(value).ok());
    let request_id = if values.next().is_none() {
        first
            .filter(|value| config.accepts_request_id(value))
            .unwrap_or_else(|| config.generate_request_id())
    } else {
        config.generate_request_id()
    };
    let header_value = HeaderValue::from_str(request_id.as_str())
        .expect("validated request ID is always a header value");
    headers.insert(config.request_id_header.clone(), header_value);
    request_id
}

fn random_request_id() -> RequestId {
    RequestId::parse(&Uuid::new_v4().simple().to_string())
        .expect("UUID simple formatting satisfies the request-ID contract")
}

fn select_trace_context(headers: &HeaderMap) -> Option<TraceContext> {
    let mut parents = headers.get_all("traceparent").iter();
    let first = parents.next()?.to_str().ok()?;
    if parents.next().is_some() {
        return None;
    }
    let trace = parse_traceparent(first)?;

    let states = headers
        .get_all("tracestate")
        .iter()
        .map(HeaderValue::to_str)
        .collect::<Result<Vec<_>, _>>();
    let tracestate = states.ok().and_then(parse_tracestate);
    Some(trace.with_tracestate(tracestate))
}

fn request_span(context: &RequestContext) -> Span {
    if let Some(trace) = context.trace_context() {
        tracing::info_span!(
            target: "axum_observability::request",
            "request",
            request_id = %context.request_id(),
            correlation_id = context.correlation_id(),
            trace_id = trace.trace_id(),
            parent_id = trace.parent_id(),
            trace_flags = u64::from(trace.flags()),
            trace_sampled = trace.sampled(),
        )
    } else {
        tracing::info_span!(
            target: "axum_observability::request",
            "request",
            request_id = %context.request_id(),
            correlation_id = context.correlation_id(),
            trace_id = tracing::field::Empty,
            parent_id = tracing::field::Empty,
            trace_flags = tracing::field::Empty,
            trace_sampled = tracing::field::Empty,
        )
    }
}

fn default_level(status: StatusCode) -> Level {
    if status.is_server_error() {
        Level::ERROR
    } else if status.is_client_error() {
        Level::WARN
    } else {
        Level::INFO
    }
}

#[derive(Debug)]
struct RequestMetadata {
    method: String,
    path: Option<String>,
    path_template: Option<String>,
    operation_id: Option<String>,
    peer_ip: Option<String>,
    user_agent: Option<String>,
}

impl RequestMetadata {
    fn from_request(request: &Request<Body>, config: &ObservabilityConfig) -> Self {
        Self {
            method: request.method().to_string(),
            path: config.raw_path.then(|| request.uri().path().to_owned()),
            path_template: request
                .extensions()
                .get::<MatchedPath>()
                .map(|path| path.as_str().to_owned()),
            operation_id: request
                .extensions()
                .get::<OperationId>()
                .map(|operation| operation.as_str().to_owned()),
            peer_ip: peer_ip(request, config),
            user_agent: config
                .user_agent
                .then(|| exactly_one_header(request.headers(), &USER_AGENT))
                .flatten(),
        }
    }
}

#[cfg(feature = "peer-ip")]
fn peer_ip(request: &Request<Body>, config: &ObservabilityConfig) -> Option<String> {
    if !config.peer_ip {
        return None;
    }
    request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|connect| connect.0.ip().to_string())
}

#[cfg(not(feature = "peer-ip"))]
fn peer_ip(_request: &Request<Body>, _config: &ObservabilityConfig) -> Option<String> {
    None
}

fn exactly_one_header(headers: &HeaderMap, name: &HeaderName) -> Option<String> {
    let mut values = headers.get_all(name).iter();
    let first = values.next()?.to_str().ok()?;
    values.next().is_none().then(|| first.to_owned())
}

#[derive(Debug, Serialize)]
pub(crate) struct AccessRecord {
    request_id: String,
    correlation_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    trace_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    trace_flags: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    trace_sampled: Option<bool>,
    method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    path_template: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    operation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<u16>,
    duration_ms: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    peer_ip: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    user_agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    terminal_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    enrichment: BTreeMap<String, Value>,
}

struct TerminalState {
    metadata: RequestMetadata,
    request_context: RequestContext,
    started: Instant,
    status: Option<StatusCode>,
    config: ObservabilityConfig,
    span: Span,
    enrichment: BTreeMap<String, Value>,
}

struct TerminalGuard {
    state: Option<TerminalState>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TerminalOutcome {
    Completed,
    ServiceError,
    BodyError,
    ResponseDropped,
}

impl TerminalOutcome {
    const fn record_fields(self) -> (Option<&'static str>, Option<&'static str>) {
        match self {
            Self::Completed => (None, None),
            Self::ServiceError => (Some("service_error"), Some("downstream service failed")),
            Self::BodyError => (Some("body_error"), Some("response body failed")),
            Self::ResponseDropped => (Some("response_dropped"), None),
        }
    }
}

impl TerminalGuard {
    fn new(
        metadata: RequestMetadata,
        request_context: RequestContext,
        started: Instant,
        config: ObservabilityConfig,
        span: Span,
        enrichment: BTreeMap<String, Value>,
    ) -> Self {
        Self {
            state: Some(TerminalState {
                metadata,
                request_context,
                started,
                status: None,
                config,
                span,
                enrichment,
            }),
        }
    }

    fn request_id(&self) -> &RequestId {
        self.state
            .as_ref()
            .expect("terminal guard has not completed")
            .request_context
            .request_id()
    }

    fn set_status(&mut self, status: StatusCode) {
        if let Some(state) = &mut self.state {
            state.status = Some(status);
        }
    }

    fn set_operation_id(&mut self, operation_id: &OperationId) {
        if let Some(state) = &mut self.state {
            state.metadata.operation_id = Some(operation_id.as_str().to_owned());
        }
    }

    fn finish(&mut self, outcome: TerminalOutcome) {
        let Some(state) = self.state.take() else {
            return;
        };
        let finished =
            catch_unwind(AssertUnwindSafe(|| (state.config.clock)())).unwrap_or(state.started);
        let duration = finished.saturating_duration_since(state.started);
        let trace = state.request_context.trace_context();
        let (terminal_reason, error) = outcome.record_fields();
        let record = AccessRecord {
            request_id: state.request_context.request_id().as_str().to_owned(),
            correlation_id: state.request_context.correlation_id().to_owned(),
            trace_id: trace.map(|trace| trace.trace_id().to_owned()),
            parent_id: trace.map(|trace| trace.parent_id().to_owned()),
            trace_flags: trace.map(TraceContext::flags),
            trace_sampled: trace.map(TraceContext::sampled),
            method: state.metadata.method,
            path: state.metadata.path,
            path_template: state.metadata.path_template,
            operation_id: state.metadata.operation_id,
            status: state.status.map(|status| status.as_u16()),
            duration_ms: duration.as_secs_f64() * 1_000.0,
            peer_ip: state.metadata.peer_ip,
            user_agent: state.metadata.user_agent,
            terminal_reason: terminal_reason.map(str::to_owned),
            error: error.map(str::to_owned),
            enrichment: state.enrichment,
        };
        let mapped_level = |status| {
            catch_unwind(AssertUnwindSafe(|| (state.config.level_mapper)(status)))
                .unwrap_or_else(|_| default_level(status))
        };
        let level = match outcome {
            TerminalOutcome::Completed => {
                mapped_level(state.status.expect("completed response has a status"))
            }
            TerminalOutcome::ServiceError | TerminalOutcome::BodyError => Level::ERROR,
            TerminalOutcome::ResponseDropped => state.status.map_or(Level::WARN, mapped_level),
        };
        let serialized = serde_json::to_string(&record).expect("access record is serializable");
        state.span.in_scope(|| emit_access(level, &serialized));
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        self.finish(TerminalOutcome::ResponseDropped);
    }
}

fn emit_access(level: Level, serialized: &str) {
    match level {
        Level::ERROR => tracing::event!(
            target: "axum_observability::access",
            Level::ERROR,
            message = "request completed",
            "obs.record" = serialized
        ),
        Level::WARN => tracing::event!(
            target: "axum_observability::access",
            Level::WARN,
            message = "request completed",
            "obs.record" = serialized
        ),
        Level::INFO => tracing::event!(
            target: "axum_observability::access",
            Level::INFO,
            message = "request completed",
            "obs.record" = serialized
        ),
        Level::DEBUG => tracing::event!(
            target: "axum_observability::access",
            Level::DEBUG,
            message = "request completed",
            "obs.record" = serialized
        ),
        Level::TRACE => tracing::event!(
            target: "axum_observability::access",
            Level::TRACE,
            message = "request completed",
            "obs.record" = serialized
        ),
    }
}

pin_project! {
    struct ObservedBody {
        #[pin]
        body: Body,
        guard: TerminalGuard,
    }
}

impl ObservedBody {
    fn new(body: Body, mut guard: TerminalGuard) -> Self {
        if body.is_end_stream() {
            guard.finish(TerminalOutcome::Completed);
        }
        Self { body, guard }
    }
}

impl HttpBody for ObservedBody {
    type Data = Bytes;
    type Error = axum::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let mut this = self.project();
        match this.body.as_mut().poll_frame(context) {
            Poll::Ready(None) => {
                this.guard.finish(TerminalOutcome::Completed);
                Poll::Ready(None)
            }
            Poll::Ready(Some(Err(error))) => {
                this.guard.finish(TerminalOutcome::BodyError);
                Poll::Ready(Some(Err(error)))
            }
            Poll::Ready(Some(Ok(frame))) => {
                if this.body.is_end_stream() {
                    this.guard.finish(TerminalOutcome::Completed);
                }
                Poll::Ready(Some(Ok(frame)))
            }
            Poll::Pending => Poll::Pending,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.body.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.body.size_hint()
    }
}
