use cuervo_client::config::ClientConfig;
use cuervo_client::error::ClientError;

#[test]
fn client_config_default() {
    let config = ClientConfig::default();
    assert_eq!(config.base_url, "http://127.0.0.1:9849");
    assert!(config.auth_token.is_empty());
    assert_eq!(config.timeout.as_secs(), 30);
    assert_eq!(config.max_retries, 3);
}

#[test]
fn client_config_new() {
    let config = ClientConfig::new("http://localhost:8080", "my-token");
    assert_eq!(config.base_url, "http://localhost:8080");
    assert_eq!(config.auth_token, "my-token");
}

#[test]
fn client_config_ws_url() {
    let config = ClientConfig::new("http://127.0.0.1:9849", "abc123");
    let ws_url = config.ws_url();
    assert_eq!(ws_url, "ws://127.0.0.1:9849/ws/events?token=abc123");
}

#[test]
fn client_config_ws_url_https() {
    let config = ClientConfig::new("https://remote.host:443", "token");
    let ws_url = config.ws_url();
    assert_eq!(ws_url, "wss://remote.host:443/ws/events?token=token");
}

#[test]
fn client_config_api_url() {
    let config = ClientConfig::new("http://127.0.0.1:9849", "tok");
    assert_eq!(config.api_url("agents"), "http://127.0.0.1:9849/api/v1/agents");
    assert_eq!(
        config.api_url("/agents/123"),
        "http://127.0.0.1:9849/api/v1/agents/123"
    );
}

#[test]
fn client_config_api_url_trailing_slash() {
    let config = ClientConfig::new("http://127.0.0.1:9849/", "tok");
    assert_eq!(
        config.api_url("system/status"),
        "http://127.0.0.1:9849/api/v1/system/status"
    );
}

#[test]
fn client_creation() {
    let config = ClientConfig::new("http://127.0.0.1:9849", "token");
    let client = cuervo_client::CuervoClient::new(config).unwrap();
    assert_eq!(client.config().base_url, "http://127.0.0.1:9849");
    assert_eq!(client.config().auth_token, "token");
}

#[test]
fn client_error_display() {
    let err = ClientError::NotConnected;
    assert_eq!(format!("{err}"), "not connected");

    let err = ClientError::Timeout;
    assert_eq!(format!("{err}"), "timeout");

    let err = ClientError::ConnectionFailed("refused".into());
    assert!(format!("{err}").contains("refused"));
}

#[test]
fn client_error_from_api_error() {
    let api_err = cuervo_api::error::ApiError::not_found("missing");
    let client_err: ClientError = api_err.into();
    match client_err {
        ClientError::Api(e) => {
            assert_eq!(e.code, cuervo_api::error::ErrorCode::NotFound);
        }
        _ => panic!("expected Api variant"),
    }
}
