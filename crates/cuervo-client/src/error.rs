use cuervo_api::error::ApiError;

/// Client-side errors.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("WebSocket error: {0}")]
    WebSocket(String),

    #[error("API error: {0}")]
    Api(ApiError),

    #[error("connection failed: {0}")]
    ConnectionFailed(String),

    #[error("not connected")]
    NotConnected,

    #[error("deserialization error: {0}")]
    Deserialize(#[from] serde_json::Error),

    #[error("URL parse error: {0}")]
    UrlParse(#[from] url::ParseError),

    #[error("timeout")]
    Timeout,
}

impl From<ApiError> for ClientError {
    fn from(e: ApiError) -> Self {
        ClientError::Api(e)
    }
}
