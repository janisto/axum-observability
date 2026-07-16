/// Validated inbound W3C trace context.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraceContext {
    trace_id: String,
    parent_id: String,
    flags: u8,
    traceparent: String,
    tracestate: Option<String>,
}

impl TraceContext {
    pub(crate) fn new(trace_id: String, parent_id: String, flags: u8, traceparent: String) -> Self {
        Self {
            trace_id,
            parent_id,
            flags,
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
    request_id: String,
    correlation_id: String,
    trace_context: Option<TraceContext>,
}

impl<S> axum::extract::FromRequestParts<S> for RequestContext
where
    S: Send + Sync,
{
    type Rejection = axum::http::StatusCode;

    async fn from_request_parts(
        parts: &mut axum::http::request::Parts,
        _state: &S,
    ) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<Self>()
            .cloned()
            .ok_or(axum::http::StatusCode::INTERNAL_SERVER_ERROR)
    }
}

impl RequestContext {
    pub(crate) fn new(request_id: String, trace_context: Option<TraceContext>) -> Self {
        let correlation_id = trace_context
            .as_ref()
            .map_or_else(|| request_id.clone(), |trace| trace.trace_id.clone());
        Self {
            request_id,
            correlation_id,
            trace_context,
        }
    }

    /// Validated or generated request identifier.
    #[must_use]
    pub fn request_id(&self) -> &str {
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
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperationId(String);

impl OperationId {
    /// Creates an operation identifier.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the operation identifier.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<String> for OperationId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for OperationId {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::OperationId;

    #[test]
    fn operation_id_preserves_owned_and_borrowed_values() {
        assert_eq!(OperationId::new("create-item").as_str(), "create-item");
        assert_eq!(
            OperationId::from(String::from("list-items")).as_str(),
            "list-items"
        );
    }
}
