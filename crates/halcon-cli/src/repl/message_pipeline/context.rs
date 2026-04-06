//! Stage 2: Context Assembly — multimodal analysis, system prompt, MCP init, tool defs.
//!
//! # Xiyo Comparison
//!
//! Xiyo's context assembly is split between `fetchSystemPromptParts()` and
//! `processUserInput()` in QueryEngine.ts (lines 294, 502). Halcon consolidates
//! all context sources (instructions, memory, dev ecosystem, media, MCP) into
//! a single typed output.
//!
//! Key improvements over Xiyo:
//! - **DevGateway context**: Real-time git/IDE/CI integration (Xiyo has none)
//! - **Media context pipeline**: 5-phase multimodal with adaptive timeouts
//! - **MCP lazy init**: Auto-discover tools from MCP servers
//!
//! # Side Effects
//!
//! - Network I/O for multimodal API calls (parallel, with timeout)
//! - Filesystem reads for media files
//! - MCP server connections (idempotent)
//! - DevGateway git/IDE polling (spawn_blocking)

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;

use halcon_core::traits::ContextQuery;
use halcon_core::types::{
    AppConfig, ContentBlock, ImageMediaType, ImageSource, MessageContent, ModelRequest, Role,
    Session,
};
use halcon_tools::ToolRegistry;

use crate::render::sink::RenderSink;
use crate::repl::bridges::dev_gateway::DevGateway;
use crate::repl::bridges::mcp_manager::McpResourceManager;
use crate::repl::context_manager::ContextManager;

/// Output of the Context Assembly stage.
pub struct ContextOutput {
    /// Fully assembled model request (messages, tools, system prompt, config).
    pub request: ModelRequest,
    /// Working directory (resolved from config or cwd).
    pub working_dir: String,
    /// Whether media was injected into session messages (Text → Blocks).
    pub had_media: bool,
}

/// Stage 2: Context Assembly.
///
/// Assembles the complete ModelRequest from all context sources:
/// system prompt, user/dev/media context, tool definitions.
pub struct ContextStage;

impl ContextStage {
    /// Execute context assembly.
    ///
    /// # Phases
    /// 1. Multimodal media analysis (if paths non-empty)
    /// 2. MCP lazy initialization (idempotent)
    /// 3. Cenzontle MCP bridge (feature-gated)
    /// 4. System prompt assembly via ContextManager
    /// 5. Context injection (user, dev ecosystem, media)
    /// 6. ModelRequest construction
    #[allow(clippy::too_many_arguments)]
    pub async fn execute(
        input: &str,
        session: &mut Session,
        media_paths: &[PathBuf],
        multimodal: &Option<Arc<halcon_multimodal::MultimodalSubsystem>>,
        context_manager: &mut Option<ContextManager>,
        mcp_manager: &mut McpResourceManager,
        tool_registry: &mut ToolRegistry,
        dev_gateway: &DevGateway,
        config: &AppConfig,
        model: &str,
        user_display_name: &str,
        #[cfg(feature = "cenzontle-agents")] provider_name: &str,
        #[cfg(feature = "cenzontle-agents")] registry: &halcon_providers::ProviderRegistry,
        sink: &dyn RenderSink,
    ) -> Result<ContextOutput> {
        // ── Multimodal media analysis ──
        let (media_context, had_media) = if !media_paths.is_empty() {
            if let Some(mm_sys) = multimodal.clone() {
                Self::analyze_media(
                    input,
                    session,
                    media_paths,
                    &mm_sys,
                    config.multimodal.api_timeout_ms,
                    sink,
                )
                .await
            } else {
                (String::new(), false)
            }
        } else {
            (String::new(), false)
        };

        // ── Resolve working directory ──
        let working_dir = config
            .general
            .working_directory
            .as_ref()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| {
                std::env::current_dir()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string()
            });

        // ── System prompt assembly via ContextManager ──
        let system_prompt = if let Some(ref mut cm) = context_manager {
            let context_query = ContextQuery {
                working_directory: working_dir.clone(),
                user_message: Some(input.to_string()),
                token_budget: config.general.max_tokens as usize,
            };
            let assembled = cm.assemble(&context_query).await;
            assembled.system_prompt
        } else {
            None
        };

        // ── MCP lazy initialization ──
        if !mcp_manager.is_initialized() && mcp_manager.has_servers() {
            let results = mcp_manager.ensure_initialized(tool_registry).await;
            for (server, result) in &results {
                match result {
                    Ok(()) => sink.info(&format!("[mcp] Server '{server}' connected")),
                    Err(e) => sink.info(&format!("[mcp] Server '{server}' failed to connect: {e}")),
                }
            }
            let n = mcp_manager.registered_tool_count();
            if n > 0 {
                tracing::info!(tool_count = n, "MCP tools registered into agent loop");
            }
        }

        // ── Cenzontle MCP bridge (feature-gated) ──
        #[cfg(feature = "cenzontle-agents")]
        {
            use crate::repl::bridges::CenzontleMcpManager;

            static CENZONTLE_MCP_INIT: std::sync::Once = std::sync::Once::new();
            let should_init = provider_name == "cenzontle" || registry.get("cenzontle").is_some();
            if should_init {
                CENZONTLE_MCP_INIT.call_once(|| {
                    tracing::debug!("Cenzontle MCP bridge: will initialize on next opportunity");
                });
                if let Some(token) = crate::commands::cenzontle::resolve_access_token_silent() {
                    let client = Arc::new(halcon_providers::CenzontleAgentClient::new(token, None));
                    let mut bridge = CenzontleMcpManager::new(client);
                    bridge.ensure_initialized(tool_registry).await;
                    if bridge.tool_count() > 0 {
                        sink.info(&format!(
                            "[cenzontle] {} MCP tools registered",
                            bridge.tool_count()
                        ));
                    }
                }
            }
        }

        // ── Build ModelRequest ──
        let tool_defs = tool_registry.tool_definitions();
        let mut request = ModelRequest {
            model: model.to_string(),
            messages: session.messages.clone(),
            tools: tool_defs,
            max_tokens: Some(config.general.max_tokens),
            temperature: Some(config.general.temperature),
            system: system_prompt,
            stream: true,
        };

        // ── Inject user context (idempotent via marker) ──
        const USER_CTX_MARKER: &str = "## User Context";
        if let Some(ref mut sys) = request.system {
            if !sys.contains(USER_CTX_MARKER) {
                sys.push_str(&format!(
                    "\n\n{USER_CTX_MARKER}\nUser: {}\nDirectory: {}\nPlatform: {}",
                    user_display_name,
                    working_dir,
                    std::env::consts::OS,
                ));
            }
        }

        // ── Inject dev ecosystem context (refreshed every round) ──
        {
            const DEV_ECO_MARKER: &str = "## Dev Ecosystem Context";
            if let Some(ref mut sys) = request.system {
                if let Some(idx) = sys.find(&format!("\n\n{DEV_ECO_MARKER}")) {
                    sys.truncate(idx);
                } else if sys.starts_with(DEV_ECO_MARKER) {
                    sys.clear();
                }
                let dev_ctx = dev_gateway.build_context().await;
                let dev_md = dev_ctx.as_markdown();
                if !dev_md.is_empty() {
                    sys.push_str(&format!("\n\n{dev_md}"));
                }
            }
        }

        // ── Inject media context (idempotent via marker) ──
        const MEDIA_CTX_MARKER: &str = "## Media Context";
        if !media_context.is_empty() {
            if let Some(ref mut sys) = request.system {
                if !sys.contains(MEDIA_CTX_MARKER) {
                    sys.push_str(&format!("\n\n{}", media_context));
                }
            } else {
                request.system = Some(media_context);
            }
        }

        // Sync messages if media modified the session.
        if had_media {
            request.messages = session.messages.clone();
        }

        Ok(ContextOutput {
            request,
            working_dir,
            had_media,
        })
    }

    /// Analyze media files referenced in user message (5-phase pipeline).
    ///
    /// Returns `(media_context_markdown, had_image_blocks_injected)`.
    pub async fn analyze_media(
        input: &str,
        session: &mut Session,
        paths: &[PathBuf],
        mm_sys: &Arc<halcon_multimodal::MultimodalSubsystem>,
        base_timeout_ms: u64,
        sink: &dyn RenderSink,
    ) -> (String, bool) {
        sink.media_analysis_started(paths.len());
        let session_id = session.id.to_string();
        let mut ctx = String::from("## Media Context\n");
        let mut img_blocks: Vec<ContentBlock> = Vec::new();
        let mut analyzed_count: usize = 0;
        let mut total_tokens_estimated: u32 = 0;

        // Phase 1: Sequential file reads.
        let mut read_data: Vec<Option<(PathBuf, Vec<u8>)>> = Vec::with_capacity(paths.len());
        for path in paths {
            match tokio::fs::read(path).await {
                Ok(data) => read_data.push(Some((path.clone(), data))),
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "Cannot read media file");
                    read_data.push(None);
                }
            }
        }

        // Phase 2: Audio fallback check + build analysis items.
        let mut analysis_items: Vec<(usize, PathBuf, Vec<u8>)> = Vec::new();
        let mut ctx_parts: Vec<Option<String>> = vec![None; paths.len()];

        for (idx, entry) in read_data.into_iter().enumerate() {
            let Some((path, data)) = entry else { continue };
            let fname = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.to_string_lossy().into_owned());
            let modality_hint = halcon_multimodal::MultimodalSubsystem::peek_modality(&data);
            if modality_hint == "audio" && !mm_sys.supports_audio() {
                sink.warning(
                    &format!(
                        "[media] Audio transcription unavailable for '{fname}': set OPENAI_API_KEY"
                    ),
                    Some("Audio analysis requires OpenAI Whisper — OPENAI_API_KEY not set"),
                );
                let native_meta = halcon_multimodal::MultimodalSubsystem::native_audio_description(
                    &data,
                )
                .unwrap_or_else(|| {
                    "Audio file — transcription unavailable. Set OPENAI_API_KEY to enable Whisper."
                        .into()
                });
                ctx_parts[idx] = Some(format!("\n### {fname}\n{native_meta}\n"));
                continue;
            }
            analysis_items.push((idx, path, data));
        }

        // Phase 3: Parallel media analysis (network-bound).
        let analysis_futures: Vec<_> = analysis_items
            .into_iter()
            .map(|(idx, path, data)| {
                let mm = Arc::clone(mm_sys);
                let sid = session_id.clone();
                let path_str = path.to_string_lossy().to_string();
                let fname = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path_str.clone());
                let size_mb = (data.len() / (1024 * 1024)) as u64;
                let adaptive_ms = base_timeout_ms
                    .saturating_add(5_000)
                    .saturating_add(size_mb * 2_000);
                async move {
                    let timeout_dur = std::time::Duration::from_millis(adaptive_ms);
                    let result = tokio::time::timeout(
                        timeout_dur,
                        mm.analyze_bytes_with_provenance(
                            &data,
                            None,
                            Some(sid),
                            Some(path_str.clone()),
                        ),
                    )
                    .await;
                    let outcome = match result {
                        Ok(Ok(analysis)) => {
                            let img_block = if analysis.modality == "image" {
                                use base64::Engine as _;
                                let encoded =
                                    base64::engine::general_purpose::STANDARD.encode(&data);
                                let media_type = ImageMediaType::from_magic(&data)
                                    .unwrap_or(ImageMediaType::Jpeg);
                                Some(ContentBlock::Image {
                                    source: ImageSource::Base64 {
                                        media_type,
                                        data: encoded,
                                    },
                                })
                            } else {
                                None
                            };
                            Ok((analysis, img_block))
                        }
                        Ok(Err(e)) => Err(format!("{e}")),
                        Err(_elapsed) => Err(format!("timed out after {}s", adaptive_ms / 1_000)),
                    };
                    (idx, fname, path_str, outcome)
                }
            })
            .collect();

        let analysis_results = futures::future::join_all(analysis_futures).await;

        // Phase 4: Collect results (sequential — needs sink).
        for (idx, fname, path_str, outcome) in analysis_results {
            match outcome {
                Ok((analysis, img_block)) => {
                    ctx_parts[idx] = Some(format!("\n### {fname}\n{}\n", analysis.description));
                    if let Some(block) = img_block {
                        img_blocks.push(block);
                    }
                    analyzed_count += 1;
                    total_tokens_estimated += analysis.token_estimate;
                    sink.media_analysis_complete(&fname, analysis.token_estimate);
                    tracing::info!(
                        path = %path_str,
                        modality = %analysis.modality,
                        tokens = analysis.token_estimate,
                        "Multimodal analysis complete"
                    );
                }
                Err(msg) => {
                    sink.warning(&format!("Media analysis failed for '{fname}': {msg}"), None);
                    tracing::warn!(path = %path_str, error = %msg, "Media analysis error");
                }
            }
        }

        // Phase 5: Assemble in original path order.
        for part in ctx_parts.into_iter().flatten() {
            ctx.push_str(&part);
        }
        if analyzed_count > 0 {
            sink.info(&format!(
                "[media] {analyzed_count}/{} file{} analyzed — ~{total_tokens_estimated} tokens added to context",
                paths.len(),
                if paths.len() == 1 { "" } else { "s" },
            ));
        }

        // Update session message: Text → Blocks(text + images).
        let had_media = if !img_blocks.is_empty() {
            if let Some(last) = session.messages.last_mut() {
                if matches!(last.role, Role::User) {
                    let mut blocks = vec![ContentBlock::Text {
                        text: input.to_string(),
                    }];
                    blocks.extend(img_blocks);
                    last.content = MessageContent::Blocks(blocks);
                    true
                } else {
                    false
                }
            } else {
                false
            }
        } else {
            false
        };

        let media_context = if ctx != "## Media Context\n" {
            ctx
        } else {
            String::new()
        };

        (media_context, had_media)
    }
}
