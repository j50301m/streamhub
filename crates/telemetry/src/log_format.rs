//! Structured JSON log formatter that captures span fields and injects the
//! active OpenTelemetry trace_id so Loki can link logs to Tempo traces.

use opentelemetry::trace::TraceContextExt;
use std::fmt;
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Subscriber};
use tracing_opentelemetry::OpenTelemetrySpanExt;
use tracing_subscriber::fmt::FmtContext;
use tracing_subscriber::fmt::format::{FormatEvent, FormatFields, Writer};
use tracing_subscriber::layer::{Context, Layer};
use tracing_subscriber::registry::LookupSpan;

/// Stores a span's fields as a JSON map so they can be merged into log events.
#[derive(Default)]
pub struct SpanFields(pub serde_json::Map<String, serde_json::Value>);

/// Layer that captures span fields into extensions so the formatter can read them.
pub struct SpanFieldsLayer;

impl<S> Layer<S> for SpanFieldsLayer
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else {
            return;
        };
        let mut fields = serde_json::Map::new();
        let mut visitor = JsonVisitor(&mut fields);
        attrs.record(&mut visitor);
        span.extensions_mut().insert(SpanFields(fields));
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else {
            return;
        };
        let mut ext = span.extensions_mut();
        if let Some(span_fields) = ext.get_mut::<SpanFields>() {
            let mut visitor = JsonVisitor(&mut span_fields.0);
            values.record(&mut visitor);
        }
    }
}

/// Custom JSON formatter that injects OpenTelemetry trace_id and parent span fields into every log event.
pub struct JsonWithTraceId;

impl<S, N> FormatEvent<S, N> for JsonWithTraceId
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        // Extract trace_id from the current OTel span context
        let trace_id = {
            let span = tracing::Span::current();
            let context = span.context();
            let span_context = context.span().span_context().clone();
            if span_context.is_valid() {
                format!("{:032x}", span_context.trace_id())
            } else {
                String::new()
            }
        };

        // Collect fields from all parent spans (root → leaf) + the event itself
        let mut fields = serde_json::Map::new();
        if let Some(scope) = ctx.event_scope() {
            for span in scope.from_root() {
                let ext = span.extensions();
                if let Some(span_fields) = ext.get::<SpanFields>() {
                    for (k, v) in &span_fields.0 {
                        fields.insert(k.clone(), v.clone());
                    }
                }
            }
        }
        let mut visitor = JsonVisitor(&mut fields);
        event.record(&mut visitor);

        let metadata = event.metadata();
        let record = serde_json::json!({
            "timestamp": chrono::Utc::now().to_rfc3339(),
            "level": metadata.level().to_string(),
            "target": metadata.target(),
            "trace_id": trace_id,
            "fields": fields,
        });

        writeln!(writer, "{record}")
    }
}

struct JsonVisitor<'a>(&'a mut serde_json::Map<String, serde_json::Value>);

impl tracing::field::Visit for JsonVisitor<'_> {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn fmt::Debug) {
        self.0.insert(
            field.name().to_string(),
            serde_json::json!(format!("{value:?}")),
        );
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.0
            .insert(field.name().to_string(), serde_json::json!(value));
    }

    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.0
            .insert(field.name().to_string(), serde_json::json!(value));
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.0
            .insert(field.name().to_string(), serde_json::json!(value));
    }

    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.0
            .insert(field.name().to_string(), serde_json::json!(value));
    }
}
