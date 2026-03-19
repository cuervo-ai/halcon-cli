//! Model provider adapters for Halcon CLI.
//!
//! Implements `halcon_core::traits::ModelProvider` for:
//! - Echo (development/testing)
//! - Anthropic (Claude API with SSE streaming)
//! - Ollama (local models, NDJSON streaming)
//! - OpenAI (GPT-4o, o1, o3-mini via SSE streaming)
//! - DeepSeek (OpenAI-compatible API)
//! - Gemini (Google generative AI, SSE streaming)
//! - ClaudeCode (persistent `claude` CLI subprocess via NDJSON)

pub mod anthropic;
pub mod claude_code;
mod contract;
pub mod deepseek;
pub mod echo;
pub mod gemini;
pub mod http;
pub mod ollama;
pub mod openai;
pub mod openai_compat;
pub mod registry;
pub mod replay;

// US-bedrock (PASO 2-B): AWS Bedrock provider (optional feature)
#[cfg(feature = "bedrock")]
pub mod bedrock;
// US-vertex (PASO 2-C): Google Vertex AI provider (optional feature)
#[cfg(feature = "vertex")]
pub mod vertex;
// US-foundry (PASO 2-D): Azure AI Foundry provider (always available — uses openai_compat)
pub mod azure_foundry;
// US-sso: Cenzontle AI platform provider (OAuth 2.1 PKCE via Zuclubit SSO)
pub mod cenzontle;
// Intelligent model routing (intent-based, cost-aware, fallback chain)
pub mod router;

pub use anthropic::AnthropicProvider;
pub use azure_foundry::AzureFoundryProvider;
pub use claude_code::ClaudeCodeProvider;
pub use deepseek::DeepSeekProvider;
pub use echo::EchoProvider;
pub use gemini::GeminiProvider;
pub use ollama::OllamaProvider;
pub use openai::OpenAIProvider;
pub use openai_compat::OpenAICompatibleProvider;
pub use cenzontle::CenzontleProvider;
#[allow(deprecated)]
pub use cenzontle::CenzonzleProvider; // backward-compat alias — use CenzontleProvider
pub use registry::ProviderRegistry;
pub use router::{IntentClassifier, IntelligentRouter, RoutingDecision, TaskIntent};
pub use replay::ReplayProvider;

#[cfg(feature = "bedrock")]
pub use bedrock::BedrockProvider;
#[cfg(feature = "vertex")]
pub use vertex::VertexProvider;
