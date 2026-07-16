//! Request correlation and structured terminal access logging for Axum.
//!
//! [`ObservabilityLayer`] validates or generates request IDs, accepts strict
//! W3C trace context, installs a [`RequestContext`] extension, and emits one
//! terminal access event after the response body completes or is abandoned.
//! [`JsonLayer`] formats those records and application `tracing` events without
//! installing a global subscriber.

#![forbid(unsafe_code)]

mod context;
mod formatter;
mod middleware;
mod request_id;
mod trace_context;

pub use context::{OperationId, RequestContext, TraceContext};
pub use formatter::JsonLayer;
pub use middleware::{ObservabilityConfig, ObservabilityLayer};
pub use request_id::is_valid_request_id;
pub use trace_context::{parse_traceparent, parse_tracestate};

/// Structured logging field convention.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Preset {
    /// Provider-neutral fields using `level`.
    #[default]
    Default,
    /// Google Cloud structured logging fields using `severity` and
    /// `httpRequest`.
    Gcp,
    /// AWS-oriented fields, including an X-Ray-compatible trace identifier.
    Aws,
    /// Azure-oriented operation correlation fields.
    Azure,
}
