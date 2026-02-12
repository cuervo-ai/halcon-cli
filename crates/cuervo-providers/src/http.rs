//! Shared HTTP client builder for all providers.
//!
//! Configures timeouts, connection pooling, and user-agent consistently
//! so each provider doesn't duplicate this logic.

use std::time::Duration;

use cuervo_core::types::HttpConfig;

/// Build a pre-configured `reqwest::Client` from `HttpConfig`.
///
/// All providers should use this instead of `reqwest::Client::new()`.
pub fn build_client(config: &HttpConfig) -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(config.connect_timeout_secs))
        // Note: We do NOT set a global request_timeout here because SSE streaming
        // responses can legitimately run for minutes. The request_timeout is
        // enforced per-attempt at the provider level using tokio::time::timeout.
        .pool_max_idle_per_host(4)
        .user_agent(format!("cuervo-cli/{}", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("failed to build HTTP client")
}

/// Determine whether a given HTTP status code is retryable.
///
/// Retryable: 429 (rate limited), 500, 502, 503, 529 (overloaded).
pub fn is_retryable_status(status: u16) -> bool {
    matches!(status, 429 | 500 | 502 | 503 | 529)
}

/// Compute backoff delay for a retry attempt using exponential backoff.
///
/// delay = base_ms * 2^attempt, capped at 60 seconds.
/// No jitter is applied at this level (callers may add jitter).
pub fn backoff_delay(base_delay_ms: u64, attempt: u32) -> Duration {
    let delay_ms = base_delay_ms.saturating_mul(1u64 << attempt.min(6));
    let capped = delay_ms.min(60_000);
    Duration::from_millis(capped)
}

/// Compute backoff delay with ±20% jitter to prevent thundering herd.
///
/// delay = base_ms * 2^attempt * (0.8 + rand(0..0.4)), capped at 60 seconds.
pub fn backoff_delay_with_jitter(base_delay_ms: u64, attempt: u32) -> Duration {
    use rand::Rng;
    let delay_ms = base_delay_ms.saturating_mul(1u64 << attempt.min(6));
    let capped = delay_ms.min(60_000) as f64;
    // Apply ±20% jitter: multiply by random factor in [0.8, 1.2]
    let jitter_factor = 0.8 + rand::rng().random_range(0.0..0.4);
    let jittered = (capped * jitter_factor) as u64;
    Duration::from_millis(jittered.min(60_000))
}

/// Parse the `Retry-After` header value from an HTTP response.
///
/// Returns the number of seconds to wait, or `None` if the header is missing/invalid.
pub fn parse_retry_after(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_builds_client() {
        let config = HttpConfig::default();
        let _client = build_client(&config);
    }

    #[test]
    fn retryable_statuses() {
        assert!(is_retryable_status(429));
        assert!(is_retryable_status(500));
        assert!(is_retryable_status(502));
        assert!(is_retryable_status(503));
        assert!(is_retryable_status(529));
        assert!(!is_retryable_status(200));
        assert!(!is_retryable_status(400));
        assert!(!is_retryable_status(401));
        assert!(!is_retryable_status(404));
    }

    #[test]
    fn backoff_exponential() {
        assert_eq!(backoff_delay(1000, 0), Duration::from_millis(1000));
        assert_eq!(backoff_delay(1000, 1), Duration::from_millis(2000));
        assert_eq!(backoff_delay(1000, 2), Duration::from_millis(4000));
        assert_eq!(backoff_delay(1000, 3), Duration::from_millis(8000));
    }

    #[test]
    fn backoff_capped_at_60s() {
        // 1000 * 2^7 = 128_000 → capped to 60_000
        assert_eq!(backoff_delay(1000, 7), Duration::from_millis(60_000));
        assert_eq!(backoff_delay(1000, 10), Duration::from_millis(60_000));
    }

    #[test]
    fn parse_retry_after_header() {
        let mut headers = reqwest::header::HeaderMap::new();
        assert_eq!(parse_retry_after(&headers), None);

        headers.insert(
            reqwest::header::RETRY_AFTER,
            reqwest::header::HeaderValue::from_static("30"),
        );
        assert_eq!(parse_retry_after(&headers), Some(30));
    }

    #[test]
    fn parse_retry_after_invalid() {
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert(
            reqwest::header::RETRY_AFTER,
            reqwest::header::HeaderValue::from_static("not-a-number"),
        );
        assert_eq!(parse_retry_after(&headers), None);
    }

    #[test]
    fn backoff_with_jitter_stays_within_bounds() {
        for _ in 0..100 {
            let d = backoff_delay_with_jitter(1000, 0);
            // 1000ms base ±20% = 800-1200ms
            assert!(
                d.as_millis() >= 800 && d.as_millis() <= 1200,
                "jittered delay out of range: {:?}",
                d
            );
        }
    }

    #[test]
    fn backoff_with_jitter_capped_at_60s() {
        let d = backoff_delay_with_jitter(1000, 10);
        assert!(d.as_millis() <= 60_000, "should be capped at 60s: {:?}", d);
    }
}
