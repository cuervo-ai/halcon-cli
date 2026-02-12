use axum::{
    middleware,
    routing::{delete, get, post},
    Router,
};
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;

use super::auth::auth_middleware;
use super::handlers;
use super::state::AppState;
use super::ws::ws_handler;

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
        );

    Router::new()
        .nest("/api/v1", api_routes)
        .route("/ws/events", get(ws_handler))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth_middleware,
        ))
        // Health check endpoint (no auth).
        .route("/health", get(health_check))
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

/// Simple health check (no auth required).
async fn health_check() -> &'static str {
    "ok"
}
