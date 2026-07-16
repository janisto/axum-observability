use std::{
    collections::BTreeMap,
    fmt,
    future::Future,
    net::SocketAddr,
    panic::{AssertUnwindSafe, catch_unwind},
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Instant,
};

use axum::{
    body::{Body, Bytes},
    extract::{ConnectInfo, MatchedPath},
    http::{HeaderMap, HeaderName, HeaderValue, Request, Response, StatusCode, header::USER_AGENT},
};
use http_body::{Body as HttpBody, Frame, SizeHint};
use pin_project_lite::pin_project;
use serde::Serialize;
use serde_json::Value;
use tower::{Layer, Service};
use tracing::{Instrument, Level, Span};
use uuid::Uuid;

use crate::{
    JsonLayer, OperationId, Preset, RequestContext, TraceContext, is_valid_request_id,
    parse_traceparent, parse_tracestate,
};

type Generator = Arc<dyn Fn() -> Option<String> + Send + Sync>;
type Validator = Arc<dyn Fn(&str) -> bool + Send + Sync>;
type LevelMapper = Arc<dyn Fn(StatusCode) -> Level + Send + Sync>;
type Clock = Arc<dyn Fn() -> Instant + Send + Sync>;
type Enricher = Arc<dyn Fn(&RequestContext) -> BTreeMap<String, Value> + Send + Sync>;

/// Configuration for [`ObservabilityLayer`].
#[derive(Clone)]
pub struct ObservabilityConfig {
    pub(crate) preset: Preset,
    request_id_header: HeaderName,
    response_header: bool,
    generator: Generator,
    validator: Validator,
    level_mapper: LevelMapper,
    clock: Clock,
    enricher: Enricher,
}

impl fmt::Debug for ObservabilityConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ObservabilityConfig")
            .field("preset", &self.preset)
            .field("request_id_header", &self.request_id_header)
            .field("response_header", &self.response_header)
            .finish_non_exhaustive()
    }
}

impl Default for ObservabilityConfig {
    fn default() -> Self {
        Self {
            preset: Preset::Default,
            request_id_header: HeaderName::from_static("x-request-id"),
            response_header: true,
            generator: Arc::new(|| Some(Uuid::new_v4().simple().to_string())),
            validator: Arc::new(|_| true),
            level_mapper: Arc::new(default_level),
            clock: Arc::new(Instant::now),
            enricher: Arc::new(|_| BTreeMap::new()),
        }
    }
}

impl ObservabilityConfig {
    /// Selects the provider field convention.
    #[must_use]
    pub fn with_preset(mut self, preset: Preset) -> Self {
        self.preset = preset;
        self
    }

    /// Sets the request and response correlation header name.
    ///
    /// # Errors
    ///
    /// Returns an error when `name` is not a valid HTTP header name.
    pub fn with_request_id_header(
        mut self,
        name: impl AsRef<str>,
    ) -> Result<Self, axum::http::header::InvalidHeaderName> {
        self.request_id_header = HeaderName::try_from(name.as_ref())?;
        Ok(self)
    }

    /// Enables or disables adding the request ID response header.
    #[must_use]
    pub fn with_response_header(mut self, enabled: bool) -> Self {
        self.response_header = enabled;
        self
    }

    /// Sets a fallible request ID generator. It is tried at most twice before
    /// the crate falls back to a package-owned random identifier.
    #[must_use]
    pub fn with_request_id_generator(
        mut self,
        generator: impl Fn() -> Option<String> + Send + Sync + 'static,
    ) -> Self {
        self.generator = Arc::new(generator);
        self
    }

    /// Adds a request ID validator that may narrow, but cannot weaken, the
    /// baseline URI-unreserved policy.
    #[must_use]
    pub fn with_request_id_validator(
        mut self,
        validator: impl Fn(&str) -> bool + Send + Sync + 'static,
    ) -> Self {
        self.validator = Arc::new(validator);
        self
    }

    /// Sets the mapping from final response status to access-event level.
    #[must_use]
    pub fn with_status_level_mapper(
        mut self,
        mapper: impl Fn(StatusCode) -> Level + Send + Sync + 'static,
    ) -> Self {
        self.level_mapper = Arc::new(mapper);
        self
    }

    /// Sets a monotonic clock seam, primarily for deterministic testing.
    #[must_use]
    pub fn with_clock(mut self, clock: impl Fn() -> Instant + Send + Sync + 'static) -> Self {
        self.clock = Arc::new(clock);
        self
    }

    /// Adds controlled fields to terminal access records. Reserved package
    /// fields cannot be overwritten.
    #[must_use]
    pub fn with_access_enricher(
        mut self,
        enricher: impl Fn(&RequestContext) -> BTreeMap<String, Value> + Send + Sync + 'static,
    ) -> Self {
        self.enricher = Arc::new(enricher);
        self
    }

    /// Creates a composable JSON layer using this configuration's preset.
    #[must_use]
    pub fn json_layer<W>(&self, writer: W) -> JsonLayer<W> {
        JsonLayer::new(writer, self.preset)
    }

    fn accepts_request_id(&self, value: &str) -> bool {
        is_valid_request_id(value)
            && catch_unwind(AssertUnwindSafe(|| (self.validator)(value))).unwrap_or(false)
    }

    fn generate_request_id(&self) -> String {
        for _ in 0..2 {
            let generated = catch_unwind(AssertUnwindSafe(|| (self.generator)()))
                .ok()
                .flatten();
            if let Some(value) = generated.filter(|value| self.accepts_request_id(value)) {
                return value;
            }
        }

        Uuid::new_v4().simple().to_string()
    }
}

/// Cloneable Tower layer that installs correlation and terminal access logs.
#[derive(Clone, Debug)]
pub struct ObservabilityLayer {
    config: ObservabilityConfig,
}

impl ObservabilityLayer {
    /// Creates a layer from an explicit configuration.
    #[must_use]
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
        let request_id = select_request_id(request.headers(), &self.config);
        let trace_context = select_trace_context(request.headers());
        let request_context = RequestContext::new(request_id, trace_context);
        let metadata = RequestMetadata::from_request(&request);
        let span = request_span(&request_context);
        let started = (self.config.clock)();
        let enrichment = catch_unwind(AssertUnwindSafe(|| {
            (self.config.enricher)(&request_context)
        }))
        .unwrap_or_default();

        request.extensions_mut().insert(request_context.clone());
        let future = self.inner.call(request);
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
                            let value = HeaderValue::from_str(guard.request_id())
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
                        guard.finish(
                            Some("service_error"),
                            Some("downstream service failed".to_owned()),
                        );
                        Err(error)
                    }
                }
            }
            .instrument(span),
        )
    }
}

fn select_request_id(headers: &HeaderMap, config: &ObservabilityConfig) -> String {
    let mut values = headers.get_all(&config.request_id_header).iter();
    let first = values.next().and_then(|value| value.to_str().ok());
    if values.next().is_none()
        && let Some(value) = first.filter(|value| config.accepts_request_id(value))
    {
        return value.to_owned();
    }
    config.generate_request_id()
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
            request_id = context.request_id(),
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
            request_id = context.request_id(),
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
    path: String,
    path_template: Option<String>,
    operation_id: Option<String>,
    remote_ip: Option<String>,
    user_agent: Option<String>,
}

impl RequestMetadata {
    fn from_request(request: &Request<Body>) -> Self {
        Self {
            method: request.method().to_string(),
            path: request.uri().path().to_owned(),
            path_template: request
                .extensions()
                .get::<MatchedPath>()
                .map(|path| path.as_str().to_owned()),
            operation_id: request
                .extensions()
                .get::<OperationId>()
                .map(|operation| operation.as_str().to_owned()),
            remote_ip: request
                .extensions()
                .get::<ConnectInfo<SocketAddr>>()
                .map(|connect| connect.0.ip().to_string()),
            user_agent: exactly_one_header(request.headers(), &USER_AGENT),
        }
    }
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
    path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    path_template: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    operation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<u16>,
    duration_ms: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    remote_ip: Option<String>,
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

    fn request_id(&self) -> &str {
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

    fn finish(&mut self, terminal_reason: Option<&str>, error: Option<String>) {
        let Some(state) = self.state.take() else {
            return;
        };
        let finished =
            catch_unwind(AssertUnwindSafe(|| (state.config.clock)())).unwrap_or(state.started);
        let duration = finished.saturating_duration_since(state.started);
        let trace = state.request_context.trace_context();
        let record = AccessRecord {
            request_id: state.request_context.request_id().to_owned(),
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
            remote_ip: state.metadata.remote_ip,
            user_agent: state.metadata.user_agent,
            terminal_reason: terminal_reason.map(str::to_owned),
            error,
            enrichment: state.enrichment,
        };
        let level = state.status.map_or(Level::ERROR, |status| {
            catch_unwind(AssertUnwindSafe(|| (state.config.level_mapper)(status)))
                .unwrap_or_else(|_| default_level(status))
        });
        let serialized = serde_json::to_string(&record).expect("access record is serializable");
        state.span.in_scope(|| emit_access(level, &serialized));
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        self.finish(Some("response_dropped"), None);
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
            guard.finish(None, None);
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
        let this = self.project();
        match this.body.poll_frame(context) {
            Poll::Ready(None) => {
                this.guard.finish(None, None);
                Poll::Ready(None)
            }
            Poll::Ready(Some(Err(error))) => {
                this.guard
                    .finish(Some("body_error"), Some("response body failed".to_owned()));
                Poll::Ready(Some(Err(error)))
            }
            other => other,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.body.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.body.size_hint()
    }
}
