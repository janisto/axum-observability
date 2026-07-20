use std::{fmt, io, io::Write, time::Duration};

use serde::Serialize;
use serde_json::{Map, Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use tracing::{Event, Subscriber, field::Visit, span::Attributes};
use tracing_subscriber::{Layer, fmt::MakeWriter, layer::Context, registry::LookupSpan};

use crate::FieldConvention;

/// A composable newline-delimited JSON `tracing` layer.
///
/// Each event is serialized as one compact JSON object followed by LF and
/// passed to the configured writer as one complete buffer.
///
/// Construct this layer through [`crate::ObservabilityConfig::json_layer`]
/// after finalizing the configuration, then use that same unchanged value for
/// the middleware. The layer snapshots the field convention when constructed;
/// later builder calls create a different configuration and do not update it.
/// The v1 direct constructor is intentionally unavailable:
///
/// ```compile_fail
/// use axum_observability::{FieldConvention, JsonLayer};
///
/// let _ = JsonLayer::new(std::io::sink(), FieldConvention::Generic);
/// ```
#[must_use]
pub struct JsonLayer<W> {
    writer: W,
    field_convention: FieldConvention,
    log_internal_errors: bool,
}

impl<W> JsonLayer<W> {
    pub(crate) const fn from_convention(writer: W, field_convention: FieldConvention) -> Self {
        Self {
            writer,
            field_convention,
            log_internal_errors: true,
        }
    }

    /// Enables or disables redacted formatter diagnostics on stderr.
    ///
    /// Diagnostics contain only the failed stage and, for writes, the
    /// [`io::ErrorKind`]. They never include an event payload or the writer's
    /// error text, and reporting does not use `tracing` or the configured
    /// writer.
    #[must_use = "the configured JSON layer must be installed on a subscriber"]
    pub fn log_internal_errors(mut self, enabled: bool) -> Self {
        self.log_internal_errors = enabled;
        self
    }
}

impl<W> fmt::Debug for JsonLayer<W> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("JsonLayer")
            .field("field_convention", &self.field_convention)
            .field("log_internal_errors", &self.log_internal_errors)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Default)]
struct RequestSpanFields(Map<String, Value>);

impl<S, W> Layer<S> for JsonLayer<W>
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
    W: for<'writer> MakeWriter<'writer> + Send + Sync + 'static,
{
    fn on_new_span(
        &self,
        attributes: &Attributes<'_>,
        id: &tracing::span::Id,
        context: Context<'_, S>,
    ) {
        if attributes.metadata().target() != "axum_observability::request" {
            return;
        }
        let mut visitor = JsonVisitor::default();
        attributes.record(&mut visitor);
        if let Some(span) = context.span(id) {
            span.extensions_mut()
                .insert(RequestSpanFields(visitor.fields));
        }
    }

    fn on_event(&self, event: &Event<'_>, context: Context<'_, S>) {
        let mut visitor = JsonVisitor::default();
        event.record(&mut visitor);
        let trusted_access_callsite = event.metadata().target() == "axum_observability::access"
            && event.metadata().module_path() == Some("axum_observability::middleware");
        let access_record = trusted_access_callsite
            .then(|| visitor.fields.remove("obs.record"))
            .flatten()
            .and_then(|value| value.as_str().map(str::to_owned))
            .and_then(|value| serde_json::from_str::<Value>(&value).ok());

        let mut request_fields = Map::new();
        if let Some(scope) = context.event_scope(event) {
            for span in scope.from_root() {
                if let Some(fields) = span.extensions().get::<RequestSpanFields>() {
                    request_fields.extend(fields.0.clone());
                }
            }
        }

        let mut output = Map::new();
        for (key, value) in visitor.fields {
            if !is_event_reserved(&key) {
                output.insert(key, value);
            }
        }
        let timestamp = match format_timestamp(OffsetDateTime::now_utc()) {
            Ok(timestamp) => timestamp,
            Err(failure) => {
                let _ = self.report_internal_failure(failure);
                return;
            }
        };
        output.insert("timestamp".to_owned(), Value::String(timestamp));
        output.insert(
            "target".to_owned(),
            Value::String(event.metadata().target().to_owned()),
        );
        let level_key = if self.field_convention == FieldConvention::Gcp {
            "severity"
        } else {
            "level"
        };
        output.insert(
            level_key.to_owned(),
            Value::String(level_name(*event.metadata().level(), self.field_convention).to_owned()),
        );

        if let Some(Value::Object(record)) = access_record {
            merge_access_record(&mut output, record, self.field_convention);
        }

        for (key, value) in request_fields {
            if is_request_field(&key) {
                output.insert(key, value);
            }
        }
        add_provider_trace_fields(&mut output, self.field_convention);

        let mut line = match serialize_json(&output) {
            Ok(line) => line,
            Err(failure) => {
                let _ = self.report_internal_failure(failure);
                return;
            }
        };
        line.push(b'\n');
        let mut writer = self.writer.make_writer_for(event.metadata());
        if let Err(error) = writer.write_all(&line) {
            let _ = self.report_internal_failure(InternalFailure::Write(error.kind()));
        }
    }
}

impl<W> JsonLayer<W> {
    #[must_use]
    fn report_internal_failure(&self, failure: InternalFailure) -> bool {
        let Some(diagnostic) = internal_diagnostic(self.log_internal_errors, failure) else {
            return false;
        };
        let _ = io::stderr().lock().write_all(diagnostic.as_bytes());
        true
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InternalFailure {
    Timestamp,
    Serialization,
    Write(io::ErrorKind),
}

fn format_timestamp(timestamp: OffsetDateTime) -> Result<String, InternalFailure> {
    timestamp
        .format(&Rfc3339)
        .map_err(|_| InternalFailure::Timestamp)
}

fn serialize_json<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>, InternalFailure> {
    serde_json::to_vec(value).map_err(|_| InternalFailure::Serialization)
}

fn internal_diagnostic(enabled: bool, failure: InternalFailure) -> Option<String> {
    if !enabled {
        return None;
    }
    let diagnostic = match failure {
        InternalFailure::Timestamp => {
            "axum-observability: JSON event dropped; stage=timestamp\n".to_owned()
        }
        InternalFailure::Serialization => {
            "axum-observability: JSON event dropped; stage=serialization\n".to_owned()
        }
        InternalFailure::Write(kind) => {
            format!("axum-observability: JSON event dropped; stage=write; kind={kind:?}\n")
        }
    };
    Some(diagnostic)
}

fn level_name(level: tracing::Level, convention: FieldConvention) -> &'static str {
    if convention != FieldConvention::Gcp {
        return level.as_str();
    }

    match level {
        tracing::Level::TRACE | tracing::Level::DEBUG => "DEBUG",
        tracing::Level::INFO => "INFO",
        tracing::Level::WARN => "WARNING",
        tracing::Level::ERROR => "ERROR",
    }
}

fn merge_access_record(
    output: &mut Map<String, Value>,
    mut record: Map<String, Value>,
    convention: FieldConvention,
) {
    let enrichment = record.remove("enrichment");
    for (key, value) in record {
        if !value.is_null() {
            output.insert(key, value);
        }
    }
    output.insert(
        "message".to_owned(),
        Value::String("request completed".to_owned()),
    );

    if let Some(Value::Object(fields)) = enrichment {
        for (key, value) in fields {
            if !is_reserved(&key) {
                output.insert(key, value);
            }
        }
    }

    if convention == FieldConvention::Gcp {
        let mut http_request = Map::new();
        copy_as(output, &mut http_request, "method", "requestMethod");
        copy_as(output, &mut http_request, "path", "requestUrl");
        copy_as(output, &mut http_request, "status", "status");
        copy_as(output, &mut http_request, "peer_ip", "remoteIp");
        copy_as(output, &mut http_request, "user_agent", "userAgent");
        if let Some(latency) = output.get("duration_ms").and_then(gcp_latency) {
            http_request.insert("latency".to_owned(), Value::String(latency));
        }
        output.insert("httpRequest".to_owned(), Value::Object(http_request));
    }
}

fn gcp_latency(value: &Value) -> Option<String> {
    const MAX_MILLISECONDS_EXCLUSIVE: u64 = 315_576_000_001_000;
    const MAX_MILLISECONDS_EXCLUSIVE_F64: f64 = 315_576_000_001_000.0;
    if let Some(milliseconds) = value.as_u64() {
        if milliseconds >= MAX_MILLISECONDS_EXCLUSIVE {
            return None;
        }
        let seconds = milliseconds / 1_000;
        let nanos = (milliseconds % 1_000) * 1_000_000;
        return Some(if nanos == 0 {
            format!("{seconds}s")
        } else {
            let fraction = format!("{nanos:09}");
            format!("{seconds}.{}s", fraction.trim_end_matches('0'))
        });
    }
    let milliseconds = value.as_f64()?;
    if !(0.0..MAX_MILLISECONDS_EXCLUSIVE_F64).contains(&milliseconds) {
        return None;
    }
    let duration = Duration::try_from_secs_f64(milliseconds / 1_000.0).ok()?;
    let seconds = duration.as_secs();
    let nanos = u64::from(duration.subsec_nanos());
    Some(if nanos == 0 {
        format!("{seconds}s")
    } else {
        let fraction = format!("{nanos:09}");
        format!("{seconds}.{}s", fraction.trim_end_matches('0'))
    })
}

fn copy_as(
    source: &Map<String, Value>,
    destination: &mut Map<String, Value>,
    source_key: &str,
    destination_key: &str,
) {
    if let Some(value) = source.get(source_key) {
        destination.insert(destination_key.to_owned(), value.clone());
    }
}

fn add_provider_trace_fields(output: &mut Map<String, Value>, convention: FieldConvention) {
    let Some(trace_id) = validated_trace_id(output) else {
        return;
    };
    match convention {
        FieldConvention::Generic => {}
        FieldConvention::Gcp => {
            output.insert(
                "logging.googleapis.com/trace".to_owned(),
                Value::String(trace_id),
            );
            if let Some(sampled) = output.get("trace_sampled").cloned() {
                output.insert("logging.googleapis.com/trace_sampled".to_owned(), sampled);
            }
        }
        FieldConvention::Aws => {
            output.insert(
                "xray_trace_id".to_owned(),
                Value::String(format!("1-{}-{}", &trace_id[..8], &trace_id[8..])),
            );
        }
        FieldConvention::Azure => {
            output.insert("operation_Id".to_owned(), Value::String(trace_id));
            if let Some(parent_id) = output.get("parent_id").cloned() {
                output.insert("operation_ParentId".to_owned(), parent_id);
            }
        }
    }
}

fn validated_trace_id(output: &Map<String, Value>) -> Option<String> {
    let trace_id = output.get("trace_id")?.as_str()?;
    let bytes = trace_id.as_bytes();
    (bytes.len() == 32
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
        && bytes.iter().any(|byte| *byte != b'0'))
    .then(|| trace_id.to_owned())
}

fn is_request_field(key: &str) -> bool {
    matches!(
        key,
        "request_id"
            | "correlation_id"
            | "trace_id"
            | "parent_id"
            | "trace_flags"
            | "trace_sampled"
            | "trace_id_random"
    )
}

fn is_reserved(key: &str) -> bool {
    is_request_field(key)
        || matches!(
            key,
            "timestamp"
                | "level"
                | "severity"
                | "target"
                | "message"
                | "method"
                | "path"
                | "path_template"
                | "operation_id"
                | "status"
                | "duration_ms"
                | "peer_ip"
                | "user_agent"
                | "terminal_reason"
                | "error"
                | "httpRequest"
                | "logging.googleapis.com/trace"
                | "logging.googleapis.com/trace_sampled"
                | "xray_trace_id"
                | "operation_Id"
                | "operation_ParentId"
                | "obs.record"
        )
}

fn is_event_reserved(key: &str) -> bool {
    is_request_field(key)
        || matches!(
            key,
            "timestamp" | "level" | "severity" | "target" | "obs.record"
        )
}

#[derive(Default)]
struct JsonVisitor {
    fields: Map<String, Value>,
}

impl Visit for JsonVisitor {
    fn record_f64(&mut self, field: &tracing::field::Field, value: f64) {
        self.fields.insert(field.name().to_owned(), json!(value));
    }

    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.fields.insert(field.name().to_owned(), json!(value));
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.fields.insert(field.name().to_owned(), json!(value));
    }

    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.fields.insert(field.name().to_owned(), json!(value));
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.fields
            .insert(field.name().to_owned(), Value::String(value.to_owned()));
    }

    fn record_error(
        &mut self,
        field: &tracing::field::Field,
        value: &(dyn std::error::Error + 'static),
    ) {
        self.fields
            .insert(field.name().to_owned(), Value::String(value.to_string()));
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn fmt::Debug) {
        self.fields
            .insert(field.name().to_owned(), Value::String(format!("{value:?}")));
    }
}

#[cfg(test)]
mod tests {
    use serde::{Serialize, Serializer};
    use serde_json::{Map, json};
    use time::{OffsetDateTime, UtcOffset};

    use super::{
        InternalFailure, format_timestamp, gcp_latency, internal_diagnostic, merge_access_record,
        serialize_json,
    };
    use crate::FieldConvention;

    struct SerializationFailure;

    impl Serialize for SerializationFailure {
        fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
        where
            S: Serializer,
        {
            Err(serde::ser::Error::custom("secret serialization detail"))
        }
    }

    #[test]
    fn diagnostics_are_suppressible_fixed_and_redacted() {
        assert_eq!(internal_diagnostic(false, InternalFailure::Timestamp), None);
        assert_eq!(
            internal_diagnostic(true, InternalFailure::Timestamp).as_deref(),
            Some("axum-observability: JSON event dropped; stage=timestamp\n")
        );
        assert_eq!(
            internal_diagnostic(true, InternalFailure::Serialization).as_deref(),
            Some("axum-observability: JSON event dropped; stage=serialization\n")
        );
        let write = internal_diagnostic(
            true,
            InternalFailure::Write(std::io::ErrorKind::PermissionDenied),
        )
        .expect("write diagnostic");
        assert_eq!(
            write,
            "axum-observability: JSON event dropped; stage=write; kind=PermissionDenied\n"
        );
        assert!(!write.contains("request"));

        let disabled = crate::ObservabilityConfig::default()
            .json_layer(std::io::sink())
            .log_internal_errors(false);
        assert!(!disabled.report_internal_failure(InternalFailure::Timestamp));
        let enabled = crate::ObservabilityConfig::default().json_layer(std::io::sink());
        assert!(enabled.report_internal_failure(InternalFailure::Timestamp));
    }

    #[test]
    fn out_of_range_timestamp_uses_the_diagnostic_path() {
        let outside_rfc3339 = OffsetDateTime::UNIX_EPOCH.to_offset(
            UtcOffset::from_hms(1, 2, 3).expect("valid offset with unsupported seconds"),
        );
        assert_eq!(
            format_timestamp(outside_rfc3339),
            Err(InternalFailure::Timestamp)
        );
    }

    #[test]
    fn serialization_failure_is_redacted_to_its_stage() {
        assert_eq!(
            serialize_json(&SerializationFailure),
            Err(InternalFailure::Serialization)
        );
        let diagnostic = internal_diagnostic(true, InternalFailure::Serialization)
            .expect("serialization diagnostic");
        assert!(!diagnostic.contains("secret serialization detail"));
    }

    #[test]
    fn gcp_latency_formats_the_maximum_protobuf_duration_without_precision_loss() {
        let mut output = Map::new();
        let record = json!({
            "method": "GET",
            "duration_ms": 315_576_000_000_000_u64,
            "enrichment": {}
        })
        .as_object()
        .expect("object fixture")
        .clone();
        merge_access_record(&mut output, record, FieldConvention::Gcp);
        assert_eq!(output["duration_ms"], json!(315_576_000_000_000_u64));
        assert_eq!(output["httpRequest"]["latency"], "315576000000s");
    }

    #[test]
    fn gcp_latency_preserves_fractional_and_near_limit_precision() {
        assert_eq!(gcp_latency(&json!(12.5)).as_deref(), Some("0.0125s"));
        assert_eq!(
            gcp_latency(&json!(315_576_000_000_999_u64)).as_deref(),
            Some("315576000000.999s")
        );
        assert_eq!(gcp_latency(&json!(-1.0)), None);
    }

    #[test]
    fn gcp_latency_omits_provider_overflow_without_zeroing_portable_duration() {
        let mut output = Map::new();
        let record = json!({
            "method": "GET",
            "duration_ms": 315_576_000_001_000_u64,
            "enrichment": {}
        })
        .as_object()
        .expect("object fixture")
        .clone();
        merge_access_record(&mut output, record, FieldConvention::Gcp);
        assert_eq!(output["duration_ms"], json!(315_576_000_001_000_u64));
        assert!(output["httpRequest"].get("latency").is_none());
    }
}
