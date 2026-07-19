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

/// W3C Trace Context level used for inbound validation and projection.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum TraceContextLevel {
    /// W3C Trace Context Level 1. This is the compatibility default.
    #[default]
    Level1,
    /// W3C Trace Context Level 2, including the random trace-ID flag.
    Level2,
}

impl TraceContextLevel {
    /// Returns the numeric W3C Trace Context level.
    #[must_use]
    pub const fn as_u8(self) -> u8 {
        match self {
            Self::Level1 => 1,
            Self::Level2 => 2,
        }
    }
}

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

/// Version of the specification-defined Google Cloud structured-stdout profile.
///
/// [`GcpProfileVersion::LATEST`] is the newest profile implemented by this
/// installed crate. Parsing an unsupported version fails instead of falling
/// back.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct GcpProfileVersion(&'static str);

impl GcpProfileVersion {
    /// Google Cloud structured-stdout profile `0.1.0`.
    pub const V0_1_0: Self = Self("0.1.0");
    /// Newest Google Cloud profile implemented by this crate.
    pub const LATEST: Self = Self::V0_1_0;

    /// Returns the semantic profile version.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl Default for GcpProfileVersion {
    fn default() -> Self {
        Self::LATEST
    }
}

impl std::fmt::Display for GcpProfileVersion {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.0)
    }
}

impl std::str::FromStr for GcpProfileVersion {
    type Err = InvalidGcpProfileVersion;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "0.1.0" => Ok(Self::V0_1_0),
            _ => Err(InvalidGcpProfileVersion),
        }
    }
}

/// Error returned when a Google Cloud profile version is unsupported.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidGcpProfileVersion;

impl std::fmt::Display for InvalidGcpProfileVersion {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("unsupported GCP profile version")
    }
}

impl std::error::Error for InvalidGcpProfileVersion {}

#[cfg(test)]
mod tests {
    use super::{FieldConvention, GcpProfileVersion, TraceContextLevel};

    #[test]
    fn generic_is_the_default_field_convention() {
        assert_eq!(FieldConvention::default(), FieldConvention::Generic);
    }

    #[test]
    fn level_one_is_the_default_trace_context_level() {
        assert_eq!(TraceContextLevel::default(), TraceContextLevel::Level1);
        assert_eq!(TraceContextLevel::Level1.as_u8(), 1);
        assert_eq!(TraceContextLevel::Level2.as_u8(), 2);
    }

    #[test]
    fn gcp_profile_version_parsing_accepts_only_supported_exact_pins() {
        let supported = "0.1.0".parse::<GcpProfileVersion>().unwrap();
        assert_eq!(supported, GcpProfileVersion::V0_1_0);
        assert_eq!(supported.as_str(), "0.1.0");
        assert_eq!(supported.to_string(), "0.1.0");

        for unsupported in ["", "0.1", "0.2.0", "latest"] {
            let error = unsupported.parse::<GcpProfileVersion>().unwrap_err();
            assert_eq!(
                error.to_string(),
                "unsupported GCP profile version",
                "wrong error for unsupported pin {unsupported:?}"
            );
        }
    }
}
