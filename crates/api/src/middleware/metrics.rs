use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;
use metrics::{counter, histogram};
use std::time::Instant;

/// Records `http_requests_total` (counter) and `http_request_duration_seconds`
/// (histogram) for every request, labelled by method / path / status.
pub async fn track_metrics(req: Request, next: Next) -> Response {
    let method = req.method().to_string();
    let path = req.uri().path().to_string();

    let start = Instant::now();
    let response = next.run(req).await;
    let duration = start.elapsed().as_secs_f64();

    let status = response.status().as_u16().to_string();

    counter!("http_requests_total", "method" => method.clone(), "path" => path.clone(), "status" => status.clone())
        .increment(1);
    histogram!("http_request_duration_seconds", "method" => method, "path" => path, "status" => status)
        .record(duration);

    response
}
