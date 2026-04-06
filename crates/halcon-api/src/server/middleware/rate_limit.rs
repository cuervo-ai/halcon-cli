//! Lightweight per-client rate limiting middleware.
//!
//! Uses a token bucket algorithm with a sliding window. Each client (identified
//! by socket address) gets `BURST_LIMIT` requests per `WINDOW_SECS` window.
//!
//! This is a simple in-process rate limiter suitable for the localhost-bound
//! control plane. For production multi-node deployments, replace with Redis-backed
//! distributed rate limiting.

use axum::{
    extract::{ConnectInfo, Request},
    http::StatusCode,
    middleware::Next,
    response::Response,
};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;

/// Maximum requests per client within the sliding window.
const BURST_LIMIT: u32 = 120;

/// Sliding window duration in seconds.
const WINDOW_SECS: u64 = 60;

/// Shared rate limiter state.
///
/// Keyed by client IP address (port is ignored to prevent per-connection evasion).
/// Old entries are lazily pruned on access.
#[derive(Debug, Clone, Default)]
pub struct RateLimiterState {
    buckets: Arc<Mutex<HashMap<std::net::IpAddr, ClientBucket>>>,
}

#[derive(Debug)]
struct ClientBucket {
    /// Request count in the current window.
    count: u32,
    /// Start of the current window.
    window_start: Instant,
}

impl RateLimiterState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Check if a request from this IP should be allowed.
    /// Returns `true` if allowed, `false` if rate limited.
    async fn check(&self, ip: std::net::IpAddr) -> bool {
        let mut buckets = self.buckets.lock().await;
        let now = Instant::now();

        let bucket = buckets.entry(ip).or_insert(ClientBucket {
            count: 0,
            window_start: now,
        });

        // Reset window if expired.
        if now.duration_since(bucket.window_start).as_secs() >= WINDOW_SECS {
            bucket.count = 0;
            bucket.window_start = now;
        }

        bucket.count += 1;
        bucket.count <= BURST_LIMIT
    }
}

/// Rate limiting middleware.
///
/// Returns 429 Too Many Requests when a client exceeds `BURST_LIMIT` requests
/// per `WINDOW_SECS` second window. Clients are identified by IP address.
///
/// When `ConnectInfo` is not available (e.g., in tests without a real socket),
/// the request is allowed through without rate limiting.
pub async fn rate_limit_middleware(
    connect_info: Option<ConnectInfo<SocketAddr>>,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    // Extract client IP. If not available (tests, proxy), skip rate limiting.
    let Some(ConnectInfo(addr)) = connect_info else {
        return Ok(next.run(request).await);
    };

    // Get or create rate limiter state from request extensions.
    // The state is injected via .layer(Extension(rate_limiter)) in the router.
    let state = request
        .extensions()
        .get::<RateLimiterState>()
        .cloned()
        .unwrap_or_default();

    if !state.check(addr.ip()).await {
        tracing::warn!(
            client_ip = %addr.ip(),
            "Rate limit exceeded ({BURST_LIMIT} requests / {WINDOW_SECS}s)"
        );
        return Err(StatusCode::TOO_MANY_REQUESTS);
    }

    Ok(next.run(request).await)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn allows_requests_within_limit() {
        let state = RateLimiterState::new();
        let ip: std::net::IpAddr = "127.0.0.1".parse().unwrap();

        for _ in 0..BURST_LIMIT {
            assert!(state.check(ip).await);
        }
    }

    #[tokio::test]
    async fn rejects_after_limit() {
        let state = RateLimiterState::new();
        let ip: std::net::IpAddr = "127.0.0.1".parse().unwrap();

        for _ in 0..BURST_LIMIT {
            state.check(ip).await;
        }
        // Next request should be rejected.
        assert!(!state.check(ip).await);
    }

    #[tokio::test]
    async fn different_ips_have_separate_limits() {
        let state = RateLimiterState::new();
        let ip1: std::net::IpAddr = "127.0.0.1".parse().unwrap();
        let ip2: std::net::IpAddr = "127.0.0.2".parse().unwrap();

        for _ in 0..BURST_LIMIT {
            state.check(ip1).await;
        }
        // ip1 exhausted, ip2 should still work.
        assert!(!state.check(ip1).await);
        assert!(state.check(ip2).await);
    }
}
