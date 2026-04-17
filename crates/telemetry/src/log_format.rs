//! Structured JSON log formatter that captures span fields and injects the
//! active OpenTelemetry trace_id so Loki can link logs to Tempo traces.

use std::fmt;
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Subscriber};
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
        // Extract trace_id from the current OTel span context. Kept in a
        // `pub(crate)` helper so a unit test can exercise the lookup without
        // having to build the full fmt subscriber pipeline.
        let trace_id = current_trace_id_hex();

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

/// Reads the current OpenTelemetry trace_id from whatever tracing span is
/// currently entered, as a 32-hex-digit string. Returns an empty string when
/// no valid OTel span context is attached.
///
/// Extracted from [`JsonWithTraceId::format_event`] so it can be exercised by
/// unit tests without running the full formatter pipeline. Loki ↔ Tempo
/// linking depends on this returning a non-empty id whenever an event fires
/// inside an instrumented handler — silent regression here is the kind of
/// failure that motivated SPEC-036.
pub(crate) fn current_trace_id_hex() -> String {
    use opentelemetry::trace::TraceContextExt;
    use tracing_opentelemetry::OpenTelemetrySpanExt;

    let span = tracing::Span::current();
    let context = span.context();
    let span_context = context.span().span_context().clone();
    if span_context.is_valid() {
        format!("{:032x}", span_context.trace_id())
    } else {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use opentelemetry::trace::TracerProvider;
    use tracing_subscriber::layer::SubscriberExt;

    /// Smoke test: inside a tracing span bound to the OpenTelemetry layer,
    /// `current_trace_id_hex` must return a non-empty 32-hex-digit id. This is
    /// the invariant `JsonWithTraceId` relies on to populate the `trace_id`
    /// field in JSON logs — without it, Loki logs cannot be joined to Tempo
    /// traces.
    #[test]
    fn current_trace_id_hex_is_populated_inside_active_span() {
        let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder().build();
        let tracer = provider.tracer("telemetry-log-format-test");
        let subscriber =
            tracing_subscriber::registry().with(tracing_opentelemetry::layer().with_tracer(tracer));

        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!("test_span");
            let _guard = span.enter();
            let trace_id = current_trace_id_hex();

            assert!(
                !trace_id.is_empty(),
                "trace_id should not be empty inside an active span"
            );
            assert_eq!(trace_id.len(), 32, "trace_id should be 32 hex digits");
            assert!(
                trace_id.chars().all(|c| c.is_ascii_hexdigit()),
                "trace_id should be hex: {trace_id}"
            );
        });
    }

    /// Outside any span, there is no OTel context, so the helper must return
    /// an empty string rather than panic. The formatter still needs to emit a
    /// log line in that case.
    #[test]
    fn current_trace_id_hex_is_empty_outside_any_span() {
        let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder().build();
        let tracer = provider.tracer("telemetry-log-format-test");
        let subscriber =
            tracing_subscriber::registry().with(tracing_opentelemetry::layer().with_tracer(tracer));

        tracing::subscriber::with_default(subscriber, || {
            assert_eq!(current_trace_id_hex(), "");
        });
    }
}
