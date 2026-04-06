use axum::{
    extract::Request,
    http::StatusCode,
    middleware::{self, Next},
    response::Response,
    routing::{delete, get, post},
    Router,
};
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;

use super::auth::auth_middleware;
use super::handlers;
use super::middleware::rate_limit::{rate_limit_middleware, RateLimiterState};
use super::state::AppState;
use super::ws::ws_handler;

/// Admin-only authentication middleware.
///
/// DECISION: Admin endpoints use HALCON_ADMIN_API_KEY as a bootstrap mechanism
/// (before RBAC JWT claims are available). The env var is checked at request time,
/// not at server startup, so operators can rotate the key without restarting.
/// Matches the Stripe/Linear pattern for admin API keys.
async fn admin_auth_middleware(request: Request, next: Next) -> Result<Response, StatusCode> {
    let expected = std::env::var("HALCON_ADMIN_API_KEY").unwrap_or_default();
    if expected.is_empty() {
        // No admin key configured → reject all admin requests for safety.
        tracing::warn!("HALCON_ADMIN_API_KEY not set — admin endpoints disabled");
        return Err(StatusCode::UNAUTHORIZED);
    }

    let provided = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "));

    match provided {
        Some(token) if token == expected => Ok(next.run(request).await),
        Some(_) => {
            tracing::warn!("invalid admin API key presented");
            Err(StatusCode::UNAUTHORIZED)
        }
        None => {
            tracing::warn!("missing Authorization: Bearer header on admin endpoint");
            Err(StatusCode::UNAUTHORIZED)
        }
    }
}

/// Build the full API router with all routes, middleware, and state.
pub fn build_router(state: AppState) -> Router {
    let api_routes = Router::new()
        // Agent endpoints
        .route("/agents", get(handlers::agents::list_agents))
        .route("/agents/:id", get(handlers::agents::get_agent))
        .route("/agents/:id", delete(handlers::agents::stop_agent))
        .route("/agents/:id/invoke", post(handlers::agents::invoke_agent))
        .route("/agents/:id/health", get(handlers::agents::agent_health))
        // Task endpoints
        .route("/tasks", get(handlers::tasks::list_tasks))
        .route("/tasks", post(handlers::tasks::submit_task))
        .route("/tasks/:id", get(handlers::tasks::get_task))
        .route("/tasks/:id", delete(handlers::tasks::cancel_task))
        // Tool endpoints
        .route("/tools", get(handlers::tools::list_tools))
        .route("/tools/:name/toggle", post(handlers::tools::toggle_tool))
        .route("/tools/:name/history", get(handlers::tools::tool_history))
        // Observability endpoints
        .route("/metrics", get(handlers::observability::get_metrics))
        // System endpoints
        .route("/system/status", get(handlers::system::get_status))
        .route("/system/shutdown", post(handlers::system::shutdown))
        // Config endpoints
        .route(
            "/system/config",
            get(handlers::config::get_config).put(handlers::config::update_config),
        )
        // Chat endpoints
        .route(
            "/chat/sessions",
            get(handlers::chat::list_sessions).post(handlers::chat::create_session),
        )
        .route(
            "/chat/sessions/:id",
            get(handlers::chat::get_session)
                .delete(handlers::chat::delete_session)
                .patch(handlers::chat::update_session),
        )
        .route(
            "/chat/sessions/:id/messages",
            get(handlers::chat::list_messages).post(handlers::chat::submit_message),
        )
        .route(
            "/chat/sessions/:id/active",
            delete(handlers::chat::cancel_active),
        )
        .route(
            "/chat/sessions/:id/permissions/:req_id",
            post(handlers::chat::resolve_permission),
        )
        // Remote-control endpoints
        .route(
            "/remote-control/sessions/:id/replan",
            post(handlers::remote_control::submit_replan),
        )
        .route(
            "/remote-control/sessions/:id/status",
            get(handlers::remote_control::get_status),
        )
        .route(
            "/remote-control/sessions/:id/context",
            post(handlers::remote_control::inject_context),
        );

    // Admin routes require the HALCON_ADMIN_API_KEY env var (bootstrap admin auth).
    // Mounted separately from the main API so the auth middleware is never accidentally
    // removed from admin endpoints in a future refactor.
    let admin_routes = Router::new()
        .route(
            "/api/v1/admin/usage/claude-code",
            get(handlers::admin::usage::claude_code_usage),
        )
        .route(
            "/api/v1/admin/usage/summary",
            get(handlers::admin::usage::usage_summary),
        )
        .layer(middleware::from_fn(admin_auth_middleware))
        .with_state(state.clone());

    // Routes that require Bearer token authentication.
    // The auth middleware is scoped to this sub-router so it is impossible for
    // a future refactor to accidentally expose protected routes without auth.
    let protected = Router::new()
        .nest("/api/v1", api_routes)
        .route("/ws/events", get(ws_handler))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ));

    // Rate limiter state shared across all requests.
    let rate_limiter = RateLimiterState::new();

    Router::new()
        // Health check is explicitly PUBLIC — no auth, no state required.
        .route("/health", get(health_check))
        .merge(protected)
        .merge(admin_routes)
        // H6: Per-client rate limiting (120 req/min per IP).
        .layer(axum::Extension(rate_limiter))
        .layer(middleware::from_fn(rate_limit_middleware))
        .layer(
            // Restrict CORS to localhost origins only.
            // This prevents cross-origin browser requests from arbitrary websites
            // while allowing egui desktop clients (no Origin header) to connect freely.
            CorsLayer::new()
                .allow_origin(AllowOrigin::predicate(|origin, _req| {
                    let b = origin.as_bytes();
                    b.starts_with(b"http://127.0.0.1")
                        || b.starts_with(b"http://localhost")
                        || b.starts_with(b"https://127.0.0.1")
                        || b.starts_with(b"https://localhost")
                }))
                .allow_methods([
                    axum::http::Method::GET,
                    axum::http::Method::POST,
                    axum::http::Method::PUT,
                    axum::http::Method::DELETE,
                    axum::http::Method::OPTIONS,
                ])
                .allow_headers([
                    axum::http::header::AUTHORIZATION,
                    axum::http::header::CONTENT_TYPE,
                    axum::http::header::ACCEPT,
                ]),
        )
        .layer(TraceLayer::new_for_http())
        // P0-C1: Prevent DoS via oversized request payloads.
        // 10 MB limit covers large code context submissions while blocking abuse.
        .layer(RequestBodyLimitLayer::new(10 * 1024 * 1024))
        .with_state(state)
}

/// Simple health check (no auth required).
async fn health_check() -> &'static str {
    "ok"
}
