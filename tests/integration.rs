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
    FieldConvention, GcpProfileVersion, MissingRequestContext, ObservabilityConfig,
    ObservabilityLayer, OperationId, RequestContext, RequestId, TraceContextLevel,
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

#[path = "integration/support.rs"]
mod support;

use support::*;

#[path = "integration/access_record.rs"]
mod access_record;
#[path = "integration/configuration.rs"]
mod configuration;
#[path = "integration/context.rs"]
mod context;
#[path = "integration/delegation.rs"]
mod delegation;
#[path = "integration/formatter.rs"]
mod formatter;
#[path = "integration/lifecycle.rs"]
mod lifecycle;
#[path = "integration/request_id.rs"]
mod request_id;
#[path = "integration/trace_context.rs"]
mod trace_context;
