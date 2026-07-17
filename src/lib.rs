//! Request correlation and structured terminal access logging for Axum.
//!
//! [`ObservabilityLayer`] validates or generates request IDs, accepts strict
//! W3C trace context, installs a [`RequestContext`] extension, and emits one
//! terminal access event after the response body completes or is abandoned.
//! [`JsonLayer`] formats those records and application `tracing` events without
//! installing a global subscriber.
//!
//! ```
//! use axum::{Router, routing::get};
//! use axum_observability::{ObservabilityConfig, ObservabilityLayer};
//! use tracing_subscriber::prelude::*;
//!
//! let config = ObservabilityConfig::default();
//! let subscriber = tracing_subscriber::registry()
//!     .with(config.json_layer(std::io::sink));
//! let app: Router = Router::new()
//!     .route("/health", get(|| async { "ok" }))
//!     .layer(ObservabilityLayer::new(config));
//!
//! # let _ = (subscriber, app);
//! ```

#![forbid(unsafe_code)]
#![warn(clippy::print_stdout)]

mod context;
mod formatter;
mod middleware;
mod request_id;
mod trace_context;

pub use context::{MissingRequestContext, OperationId, RequestContext, TraceContext};
pub use formatter::JsonLayer;
pub use middleware::{ObservabilityConfig, ObservabilityLayer, ObservabilityService};
pub use request_id::{InvalidRequestId, RequestId};

/// Structured logging field convention.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[non_exhaustive]
pub enum FieldConvention {
    /// Provider-neutral fields using `level`.
    #[default]
    Generic,
    /// Google Cloud structured logging fields using `severity` and
    /// `httpRequest`.
    Gcp,
    /// AWS-oriented fields, including an X-Ray-compatible trace identifier.
    Aws,
    /// Azure-oriented operation correlation fields.
    Azure,
}

#[cfg(test)]
mod tests {
    use super::FieldConvention;

    #[test]
    fn generic_is_the_default_field_convention() {
        assert_eq!(FieldConvention::default(), FieldConvention::Generic);
    }
}
