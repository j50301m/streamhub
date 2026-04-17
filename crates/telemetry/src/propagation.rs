use std::collections::HashMap;

use axum::http::{HeaderMap, Request};
use opentelemetry::propagation::{Extractor, Injector};
use opentelemetry::trace::TraceContextExt;
use tower_http::trace::{DefaultMakeSpan, MakeSpan};
use tracing::Level;
use tracing_opentelemetry::OpenTelemetrySpanExt;

struct TraceparentInjector {
    headers: HashMap<String, String>,
}

impl Injector for TraceparentInjector {
    fn set(&mut self, key: &str, value: String) {
        self.headers.insert(key.to_string(), value);
    }
}

struct SingleHeaderExtractor<'a> {
    traceparent: Option<&'a str>,
}

impl Extractor for SingleHeaderExtractor<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        match key {
            "traceparent" => self.traceparent,
            _ => None,
        }
    }

    fn keys(&self) -> Vec<&str> {
        if self.traceparent.is_some() {
            vec!["traceparent"]
        } else {
            Vec::new()
        }
    }
}

struct HeaderExtractor<'a> {
    headers: &'a HeaderMap,
}

impl Extractor for HeaderExtractor<'_> {
    fn get(&self, key: &str) -> Option<&str> {
        self.headers.get(key).and_then(|value| value.to_str().ok())
    }

    fn keys(&self) -> Vec<&str> {
        self.headers.keys().map(|name| name.as_str()).collect()
    }
}

fn extract_context<E: Extractor>(extractor: &E) -> Option<opentelemetry::Context> {
    let context =
        opentelemetry::global::get_text_map_propagator(|propagator| propagator.extract(extractor));

    if context.span().span_context().is_valid() {
        Some(context)
    } else {
        None
    }
}

fn log_set_parent_failure(result: Result<(), tracing_opentelemetry::SetParentError>, source: &str) {
    if let Err(error) = result {
        tracing::warn!(error = %error, source, "Failed to attach parent context to span");
    }
}

/// Serialize the current span context as a W3C `traceparent` header.
///
/// Returns `None` when there is no active OpenTelemetry context attached to
/// the current tracing span.
pub fn inject_traceparent() -> Option<String> {
    let context = tracing::Span::current().context();
    let span = context.span();
    let span_context = span.span_context();
    if !span_context.is_valid() {
        return None;
    }

    opentelemetry::global::get_text_map_propagator(|propagator| {
        let mut injector = TraceparentInjector {
            headers: HashMap::new(),
        };
        propagator.inject_context(&context, &mut injector);
        injector.headers.remove("traceparent")
    })
}

/// Parse a W3C `traceparent` string into an OpenTelemetry parent context.
///
/// Malformed or empty input is treated as missing and returns `None`.
pub fn extract_parent_context(traceparent: Option<&str>) -> Option<opentelemetry::Context> {
    let traceparent = traceparent.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        }
    })?;

    let extractor = SingleHeaderExtractor {
        traceparent: Some(traceparent),
    };
    if let Some(context) = extract_context(&extractor) {
        Some(context)
    } else {
        tracing::warn!("Ignoring malformed traceparent");
        None
    }
}

/// Attach a parent context from a `traceparent` string to an existing span.
///
/// Missing or malformed input is ignored.
pub fn set_parent_from_traceparent(span: &tracing::Span, traceparent: Option<&str>) {
    if let Some(context) = extract_parent_context(traceparent) {
        log_set_parent_failure(span.set_parent(context), "traceparent");
    }
}

/// Build the default HTTP request span and, when present, inherit parent
/// context from the incoming `traceparent` header.
pub fn http_make_span<B>(request: &Request<B>) -> tracing::Span {
    let span = DefaultMakeSpan::new().level(Level::INFO).make_span(request);
    let extractor = HeaderExtractor {
        headers: request.headers(),
    };
    if let Some(context) = extract_context(&extractor) {
        log_set_parent_failure(span.set_parent(context), "http.headers.traceparent");
    } else if request.headers().contains_key("traceparent") {
        tracing::warn!("Ignoring malformed traceparent from HTTP headers");
    }

    span
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use opentelemetry::trace::{
        SpanContext, TraceContextExt, TraceFlags, TraceState, TracerProvider,
    };
    use tracing_opentelemetry::OpenTelemetrySpanExt;
    use tracing_subscriber::layer::SubscriberExt;

    fn set_test_propagator() {
        opentelemetry::global::set_text_map_propagator(
            opentelemetry_sdk::propagation::TraceContextPropagator::new(),
        );
    }

    fn with_test_subscriber<T>(f: impl FnOnce() -> T) -> T {
        let provider = opentelemetry_sdk::trace::SdkTracerProvider::builder().build();
        let tracer = provider.tracer("telemetry-test");
        let subscriber =
            tracing_subscriber::registry().with(tracing_opentelemetry::layer().with_tracer(tracer));
        tracing::subscriber::with_default(subscriber, f)
    }

    fn trace_id_hex(span: &tracing::Span) -> String {
        format!("{:032x}", span.context().span().span_context().trace_id())
    }

    #[test]
    fn inject_and_extract_roundtrip_keeps_trace_id() {
        set_test_propagator();

        with_test_subscriber(|| {
            let parent = tracing::info_span!("parent");
            let _guard = parent.enter();

            let traceparent = inject_traceparent().expect("traceparent should exist");

            let child = tracing::info_span!("child");
            set_parent_from_traceparent(&child, Some(&traceparent));

            assert_eq!(trace_id_hex(&parent), trace_id_hex(&child));
        });
    }

    #[test]
    fn extract_parent_context_fail_soft_for_missing_and_malformed() {
        set_test_propagator();

        assert!(extract_parent_context(None).is_none());
        assert!(extract_parent_context(Some("")).is_none());
        assert!(extract_parent_context(Some("00-not-valid")).is_none());
    }

    #[test]
    fn http_make_span_inherits_incoming_traceparent() {
        set_test_propagator();

        with_test_subscriber(|| {
            let parent = tracing::info_span!("http_parent");
            let _guard = parent.enter();
            let traceparent = inject_traceparent().expect("traceparent should exist");

            let request = Request::builder()
                .uri("/health")
                .header("traceparent", traceparent)
                .body(Body::empty())
                .expect("request should build");

            let span = http_make_span(&request);
            assert_eq!(trace_id_hex(&parent), trace_id_hex(&span));
        });
    }

    #[test]
    fn http_make_span_creates_root_span_without_valid_header() {
        set_test_propagator();

        with_test_subscriber(|| {
            let request = Request::builder()
                .uri("/health")
                .header("traceparent", "00-invalid")
                .body(Body::empty())
                .expect("request should build");

            let span = http_make_span(&request);
            let span_context = span.context().span().span_context().clone();

            assert!(span_context.is_valid());
            let invalid = SpanContext::new(
                span_context.trace_id(),
                span_context.span_id(),
                TraceFlags::default(),
                false,
                TraceState::default(),
            );
            assert_eq!(span_context.trace_id(), invalid.trace_id());
        });
    }
}
