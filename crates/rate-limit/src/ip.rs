//! Client IP extraction from HTTP requests.

use axum::extract::ConnectInfo;
use axum::http::request::Parts;
use std::net::SocketAddr;

/// Resolved client IP address as a string.
///
/// Trusts the first IP in `X-Forwarded-For` (set by our nginx), falling back
/// to `ConnectInfo<SocketAddr>` when the header is absent.
#[derive(Debug, Clone)]
pub struct ClientIp(pub String);

impl ClientIp {
    /// Extract client IP from request parts.
    ///
    /// Priority:
    /// 1. First IP in `X-Forwarded-For` header (trusted nginx)
    /// 2. `ConnectInfo<SocketAddr>` peer address
    /// 3. `"unknown"` if neither is available
    pub fn from_parts(parts: &Parts) -> Self {
        // Try X-Forwarded-For first hop
        if let Some(xff) = parts.headers.get("x-forwarded-for") {
            if let Ok(value) = xff.to_str() {
                if let Some(first_ip) = value.split(',').next() {
                    let trimmed = first_ip.trim();
                    if !trimmed.is_empty() {
                        return ClientIp(trimmed.to_string());
                    }
                }
            }
        }

        // Fallback to ConnectInfo
        if let Some(connect_info) = parts.extensions.get::<ConnectInfo<SocketAddr>>() {
            return ClientIp(connect_info.0.ip().to_string());
        }

        ClientIp("unknown".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;

    fn parts_from_request(req: Request<()>) -> Parts {
        req.into_parts().0
    }

    #[test]
    fn extracts_xff_first_ip() {
        let req = Request::builder()
            .header("x-forwarded-for", "1.2.3.4, 5.6.7.8")
            .body(())
            .unwrap();
        let parts = parts_from_request(req);
        let ip = ClientIp::from_parts(&parts);
        assert_eq!(ip.0, "1.2.3.4");
    }

    #[test]
    fn extracts_xff_single_ip() {
        let req = Request::builder()
            .header("x-forwarded-for", "10.0.0.1")
            .body(())
            .unwrap();
        let parts = parts_from_request(req);
        let ip = ClientIp::from_parts(&parts);
        assert_eq!(ip.0, "10.0.0.1");
    }

    #[test]
    fn fallback_to_unknown_without_headers() {
        let req = Request::builder().body(()).unwrap();
        let parts = parts_from_request(req);
        let ip = ClientIp::from_parts(&parts);
        assert_eq!(ip.0, "unknown");
    }

    #[test]
    fn handles_empty_xff() {
        let req = Request::builder()
            .header("x-forwarded-for", "")
            .body(())
            .unwrap();
        let parts = parts_from_request(req);
        let ip = ClientIp::from_parts(&parts);
        assert_eq!(ip.0, "unknown");
    }

    #[test]
    fn trims_whitespace_in_xff() {
        let req = Request::builder()
            .header("x-forwarded-for", "  1.2.3.4  , 5.6.7.8")
            .body(())
            .unwrap();
        let parts = parts_from_request(req);
        let ip = ClientIp::from_parts(&parts);
        assert_eq!(ip.0, "1.2.3.4");
    }
}
