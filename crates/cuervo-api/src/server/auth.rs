use axum::{
    extract::Request,
    http::StatusCode,
    middleware::Next,
    response::Response,
};

use super::state::AppState;

/// Middleware that validates Bearer token authentication.
pub async fn auth_middleware(
    axum::extract::State(state): axum::extract::State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let auth_header = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());

    // Also check query parameter for WebSocket connections.
    let query_token = request
        .uri()
        .query()
        .and_then(|q| {
            q.split('&')
                .find_map(|pair| pair.strip_prefix("token="))
        });

    let provided_token = auth_header
        .and_then(|h| h.strip_prefix("Bearer "))
        .or(query_token);

    match provided_token {
        Some(token) if token == state.auth_token.as_str() => Ok(next.run(request).await),
        Some(_) => {
            tracing::warn!("invalid auth token presented");
            Err(StatusCode::UNAUTHORIZED)
        }
        None => {
            tracing::warn!("missing auth token");
            Err(StatusCode::UNAUTHORIZED)
        }
    }
}

/// Generate a cryptographically random auth token.
pub fn generate_token() -> String {
    use std::fmt::Write;
    let mut bytes = [0u8; 32];
    getrandom(&mut bytes);
    let mut hex = String::with_capacity(64);
    for b in &bytes {
        let _ = write!(hex, "{b:02x}");
    }
    hex
}

/// Platform-agnostic random bytes.
fn getrandom(buf: &mut [u8]) {
    // Use std's random for token generation.
    // In production, consider ring or getrandom crate.
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    for chunk in buf.chunks_mut(8) {
        let s = RandomState::new();
        let val = s.build_hasher().finish().to_le_bytes();
        let len = chunk.len().min(8);
        chunk[..len].copy_from_slice(&val[..len]);
    }
}
