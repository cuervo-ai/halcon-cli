//! Model provider adapters for Cuervo CLI.
//!
//! Implements `cuervo_core::traits::ModelProvider` for:
//! - Echo (development/testing)
//! - Anthropic (Claude API with SSE streaming)
//! - Ollama (local models, NDJSON streaming)
//! - OpenAI (GPT-4o, o1, o3-mini via SSE streaming)
//! - DeepSeek (OpenAI-compatible API)
//! - Gemini (Google generative AI, SSE streaming)

pub mod anthropic;
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

pub use anthropic::AnthropicProvider;
pub use deepseek::DeepSeekProvider;
pub use echo::EchoProvider;
pub use gemini::GeminiProvider;
pub use ollama::OllamaProvider;
pub use openai::OpenAIProvider;
pub use openai_compat::OpenAICompatibleProvider;
pub use registry::ProviderRegistry;
pub use replay::ReplayProvider;
