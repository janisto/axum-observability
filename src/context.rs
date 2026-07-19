use std::{error::Error, fmt};

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};

use crate::{RequestId, TraceContextLevel};

/// Validated inbound W3C trace context.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraceContext {
    trace_id: String,
    parent_id: String,
    flags: u8,
    level: TraceContextLevel,
    traceparent: String,
    tracestate: Option<String>,
}

impl TraceContext {
    pub(crate) fn new(
        trace_id: String,
        parent_id: String,
        flags: u8,
        level: TraceContextLevel,
        traceparent: String,
    ) -> Self {
        Self {
            trace_id,
            parent_id,
            flags,
            level,
            traceparent,
            tracestate: None,
        }
    }

    pub(crate) fn with_tracestate(mut self, tracestate: Option<String>) -> Self {
        self.tracestate = tracestate;
        self
    }

    /// Validated 32-character lowercase trace identifier.
    #[must_use]
    pub fn trace_id(&self) -> &str {
        &self.trace_id
    }

    /// Incoming 16-character lowercase parent identifier.
    #[must_use]
    pub fn parent_id(&self) -> &str {
        &self.parent_id
    }

    /// Raw W3C trace flags byte.
    #[must_use]
    pub const fn flags(&self) -> u8 {
        self.flags
    }

    /// Whether the sampled flag is set.
    #[must_use]
    pub const fn sampled(&self) -> bool {
        self.flags & 1 == 1
    }

    /// Selected W3C Trace Context level.
    #[must_use]
    pub const fn trace_context_level(&self) -> TraceContextLevel {
        self.level
    }

    /// Whether the caller marked the trace ID as random in Level 2 mode.
    ///
    /// Level 1 does not assign portable meaning to this flag, so it returns
    /// `None` even when bit one is set.
    #[must_use]
    pub const fn trace_id_random(&self) -> Option<bool> {
        match self.level {
            TraceContextLevel::Level1 => None,
            TraceContextLevel::Level2 => Some(self.flags & 2 == 2),
        }
    }

    /// Accepted raw `traceparent` value.
    #[must_use]
    pub fn traceparent(&self) -> &str {
        &self.traceparent
    }

    /// Accepted combined `tracestate`, when valid.
    #[must_use]
    pub fn tracestate(&self) -> Option<&str> {
        self.tracestate.as_deref()
    }
}

/// Correlation metadata installed in every observed request's extensions.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestContext {
    request_id: RequestId,
    correlation_id: String,
    trace_context: Option<TraceContext>,
}

impl<S> axum::extract::FromRequestParts<S> for RequestContext
where
    S: Send + Sync,
{
    type Rejection = MissingRequestContext;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<Self>()
            .cloned()
            .ok_or(MissingRequestContext)
    }
}

/// Rejection returned when [`RequestContext`] is extracted without the
/// observability middleware installed.
///
/// Handlers can explicitly retain the typed rejection when middleware is
/// optional during composition:
///
/// ```
/// use axum_observability::{MissingRequestContext, RequestContext};
///
/// async fn request_id(
///     context: Result<RequestContext, MissingRequestContext>,
/// ) -> Result<String, MissingRequestContext> {
///     Ok(context?.request_id().to_string())
/// }
/// # let _ = request_id;
/// ```
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct MissingRequestContext;

impl fmt::Display for MissingRequestContext {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("request context unavailable")
    }
}

impl Error for MissingRequestContext {}

impl IntoResponse for MissingRequestContext {
    fn into_response(self) -> Response {
        (StatusCode::INTERNAL_SERVER_ERROR, self.to_string()).into_response()
    }
}

impl RequestContext {
    pub(crate) fn new(request_id: RequestId, trace_context: Option<TraceContext>) -> Self {
        let correlation_id = trace_context.as_ref().map_or_else(
            || request_id.as_str().to_owned(),
            |trace| trace.trace_id.clone(),
        );
        Self {
            request_id,
            correlation_id,
            trace_context,
        }
    }

    /// Validated or generated request identifier.
    #[must_use]
    pub const fn request_id(&self) -> &RequestId {
        &self.request_id
    }

    /// Valid trace identifier, or the request identifier when no trace exists.
    #[must_use]
    pub fn correlation_id(&self) -> &str {
        &self.correlation_id
    }

    /// Validated inbound trace context.
    #[must_use]
    pub const fn trace_context(&self) -> Option<&TraceContext> {
        self.trace_context.as_ref()
    }
}

/// Stable application operation name attached to request or response
/// extensions.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct OperationId(&'static str);

impl OperationId {
    /// Creates an operation identifier from static route metadata.
    ///
    /// The value should be a stable semantic operation name. It must not
    /// contain request-derived or user-derived data. Callers are responsible
    /// for uniqueness within their application.
    ///
    /// # Panics
    ///
    /// Panics when `value` is empty.
    ///
    /// # Examples
    ///
    /// ```
    /// use axum_observability::OperationId;
    ///
    /// const LIST_ITEMS: OperationId = OperationId::from_static("list-items");
    /// assert_eq!(LIST_ITEMS.as_str(), "list-items");
    /// ```
    #[track_caller]
    #[must_use]
    pub const fn from_static(value: &'static str) -> Self {
        assert!(!value.is_empty(), "operation ID must not be empty");
        Self(value)
    }

    /// Returns the operation identifier.
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.0
    }
}

impl AsRef<str> for OperationId {
    fn as_ref(&self) -> &str {
        self.as_str()
    }
}

impl fmt::Display for OperationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::OperationId;

    #[test]
    fn operation_id_preserves_nonempty_static_values() {
        const OPERATION: OperationId = OperationId::from_static("create-item");
        assert_eq!(OPERATION.as_str(), "create-item");
        assert_eq!(OPERATION.as_ref(), "create-item");
        assert_eq!(OPERATION.to_string(), "create-item");
    }

    #[test]
    #[should_panic(expected = "operation ID must not be empty")]
    fn operation_id_rejects_empty_static_values() {
        let _ = OperationId::from_static("");
    }
}
