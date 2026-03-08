// DECISION: We use the gcp-auth crate instead of google-cloud-auth because:
// 1. gcp-auth is lighter (no proto/gRPC deps)
// 2. It supports all ADC sources: service account JSON, workload identity,
//    and metadata server — covering all real deployment scenarios.
//
// ENV:
//   GOOGLE_APPLICATION_CREDENTIALS — path to service account JSON
//   ANTHROPIC_VERTEX_PROJECT_ID    — GCP project ID (required)
//   CLOUD_ML_REGION                — region (default: us-east5)
//
// Activation: CLAUDE_CODE_USE_VERTEX=1
//
// See US-vertex (PASO 2-C).

/// GCP configuration resolved from environment variables.
#[derive(Clone, Debug)]
pub struct GcpConfig {
    pub project_id: String,
    pub region: String,
}

impl GcpConfig {
    /// Resolve from environment variables.
    ///
    /// Returns `None` if `ANTHROPIC_VERTEX_PROJECT_ID` is absent.
    pub fn from_env() -> Option<Self> {
        let project_id = std::env::var("ANTHROPIC_VERTEX_PROJECT_ID").ok()?;
        let region = std::env::var("CLOUD_ML_REGION")
            .unwrap_or_else(|_| "us-east5".to_string());
        Some(Self { project_id, region })
    }

    /// Vertex AI streamRawPredict endpoint for a given model.
    ///
    /// Format:
    /// https://{region}-aiplatform.googleapis.com/v1/projects/{project}/locations/{region}/publishers/anthropic/models/{model}:streamRawPredict
    pub fn stream_raw_predict_url(&self, model: &str) -> String {
        format!(
            "https://{region}-aiplatform.googleapis.com/v1/projects/{project}/locations/{region}/publishers/anthropic/models/{model}:streamRawPredict",
            region = self.region,
            project = self.project_id,
            model = model,
        )
    }
}

/// Obtain a GCP access token using Application Default Credentials.
///
/// ADC lookup order (same as gcp-auth):
/// 1. `GOOGLE_APPLICATION_CREDENTIALS` env var → service account JSON
/// 2. Well-known user credentials file (~/.config/gcloud/application_default_credentials.json)
/// 3. Metadata server (GCE/GKE/Cloud Run)
///
/// Returns the Bearer token string or an error string.
#[cfg(feature = "vertex")]
pub async fn get_access_token() -> Result<String, String> {
    use gcp_auth::AuthenticationManager;

    let manager = AuthenticationManager::new()
        .await
        .map_err(|e| format!("GCP ADC init failed: {e}"))?;

    let scopes = ["https://www.googleapis.com/auth/cloud-platform"];
    let token = manager
        .get_token(&scopes)
        .await
        .map_err(|e| format!("GCP token fetch failed: {e}"))?;

    Ok(token.as_str().to_string())
}

/// Stub for non-vertex builds.
#[cfg(not(feature = "vertex"))]
pub async fn get_access_token() -> Result<String, String> {
    Err("vertex feature not enabled".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gcp_config_from_env_missing_returns_none() {
        // Without ANTHROPIC_VERTEX_PROJECT_ID, from_env() must return None.
        let saved = std::env::var("ANTHROPIC_VERTEX_PROJECT_ID").ok();
        std::env::remove_var("ANTHROPIC_VERTEX_PROJECT_ID");
        let result = GcpConfig::from_env();
        // Restore
        if let Some(v) = saved {
            std::env::set_var("ANTHROPIC_VERTEX_PROJECT_ID", v);
        }
        assert!(result.is_none());
    }

    #[test]
    fn stream_raw_predict_url_format() {
        let cfg = GcpConfig {
            project_id: "my-project-123".into(),
            region: "us-east5".into(),
        };
        let url = cfg.stream_raw_predict_url("claude-sonnet-4-6");
        assert!(url.contains("us-east5-aiplatform.googleapis.com"), "URL={url}");
        assert!(url.contains("my-project-123"), "URL={url}");
        assert!(url.contains("publishers/anthropic/models/claude-sonnet-4-6"), "URL={url}");
        assert!(url.ends_with(":streamRawPredict"), "URL={url}");
    }

    #[test]
    fn default_region_is_us_east5() {
        std::env::set_var("ANTHROPIC_VERTEX_PROJECT_ID", "proj");
        std::env::remove_var("CLOUD_ML_REGION");
        let cfg = GcpConfig::from_env().unwrap();
        std::env::remove_var("ANTHROPIC_VERTEX_PROJECT_ID");
        assert_eq!(cfg.region, "us-east5");
    }
}
