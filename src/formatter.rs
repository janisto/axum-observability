use std::{fmt, io::Write};

use serde_json::{Map, Value, json};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use tracing::{Event, Subscriber, field::Visit, span::Attributes};
use tracing_subscriber::{Layer, fmt::MakeWriter, layer::Context, registry::LookupSpan};

use crate::Preset;

/// A composable newline-delimited JSON `tracing` layer.
pub struct JsonLayer<W> {
    writer: W,
    preset: Preset,
}

impl<W> JsonLayer<W> {
    /// Creates a JSON layer using the supplied writer and field preset.
    #[must_use]
    pub const fn new(writer: W, preset: Preset) -> Self {
        Self { writer, preset }
    }
}

impl<W> fmt::Debug for JsonLayer<W> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("JsonLayer")
            .field("preset", &self.preset)
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

    fn on_record(
        &self,
        id: &tracing::span::Id,
        values: &tracing::span::Record<'_>,
        context: Context<'_, S>,
    ) {
        if let Some(span) = context.span(id)
            && let Some(fields) = span.extensions_mut().get_mut::<RequestSpanFields>()
        {
            let mut visitor = JsonVisitor::default();
            values.record(&mut visitor);
            fields.0.extend(visitor.fields);
        }
    }

    fn on_event(&self, event: &Event<'_>, context: Context<'_, S>) {
        let mut visitor = JsonVisitor::default();
        event.record(&mut visitor);
        let access_record = visitor
            .fields
            .remove("obs.record")
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
        output.insert(
            "timestamp".to_owned(),
            Value::String(
                OffsetDateTime::now_utc()
                    .format(&Rfc3339)
                    .expect("RFC 3339 supports all UTC system timestamps"),
            ),
        );
        output.insert(
            "target".to_owned(),
            Value::String(event.metadata().target().to_owned()),
        );
        let level_key = if self.preset == Preset::Gcp {
            "severity"
        } else {
            "level"
        };
        output.insert(
            level_key.to_owned(),
            Value::String(event.metadata().level().as_str().to_owned()),
        );

        if let Some(Value::Object(record)) = access_record {
            merge_access_record(&mut output, record, self.preset);
        }

        for (key, value) in request_fields {
            if is_request_field(&key) {
                output.insert(key, value);
            }
        }
        add_provider_trace_fields(&mut output, self.preset);

        let mut writer = self.writer.make_writer_for(event.metadata());
        if serde_json::to_writer(&mut writer, &output).is_ok() {
            let _ = writer.write_all(b"\n");
        }
    }
}

fn merge_access_record(
    output: &mut Map<String, Value>,
    mut record: Map<String, Value>,
    preset: Preset,
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

    if preset == Preset::Gcp {
        let mut http_request = Map::new();
        copy_as(output, &mut http_request, "method", "requestMethod");
        copy_as(output, &mut http_request, "path", "requestUrl");
        copy_as(output, &mut http_request, "status", "status");
        copy_as(output, &mut http_request, "remote_ip", "remoteIp");
        copy_as(output, &mut http_request, "user_agent", "userAgent");
        if let Some(duration) = output.get("duration_ms").and_then(Value::as_f64) {
            http_request.insert(
                "latency".to_owned(),
                json!(format!("{}s", duration / 1_000.0)),
            );
        }
        output.insert("httpRequest".to_owned(), Value::Object(http_request));
    }
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

fn add_provider_trace_fields(output: &mut Map<String, Value>, preset: Preset) {
    let Some(trace_id) = output
        .get("trace_id")
        .and_then(Value::as_str)
        .map(str::to_owned)
    else {
        return;
    };
    match preset {
        Preset::Default => {}
        Preset::Gcp => {
            output.insert(
                "logging.googleapis.com/trace".to_owned(),
                Value::String(trace_id),
            );
            if let Some(sampled) = output.get("trace_sampled").cloned() {
                output.insert("logging.googleapis.com/trace_sampled".to_owned(), sampled);
            }
        }
        Preset::Aws => {
            output.insert(
                "xray_trace_id".to_owned(),
                Value::String(format!("1-{}-{}", &trace_id[..8], &trace_id[8..])),
            );
        }
        Preset::Azure => {
            output.insert("operation_Id".to_owned(), Value::String(trace_id));
            if let Some(parent_id) = output.get("parent_id").cloned() {
                output.insert("operation_ParentId".to_owned(), parent_id);
            }
        }
    }
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
                | "remote_ip"
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
            "timestamp"
                | "level"
                | "severity"
                | "target"
                | "httpRequest"
                | "logging.googleapis.com/trace"
                | "logging.googleapis.com/trace_sampled"
                | "xray_trace_id"
                | "operation_Id"
                | "operation_ParentId"
                | "obs.record"
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
