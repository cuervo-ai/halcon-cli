//! AgentBridgeImpl — implements AgentExecutor (halcon-cli) and ChatExecutor (halcon-core).
//!
//! Bridges the API layer to the full halcon agent pipeline without importing ratatui.
//! Key design choices:
//!   - System prompt injected at turn level (not per-request) to guide proactive tool use
//!   - Working directory defaults to $HOME so path discovery starts from the right place
//!   - tool_selection_enabled=true so ToolSelector reduces context noise for simple queries
//!   - All 60+ tools still sent for Mixed/ambiguous intents (the common case)

/// Default system prompt for Halcon API-mode sessions (used when the caller supplies None).
///
/// Instructs the model to:
///   1. Search proactively instead of asking the user for paths / missing info
///   2. Use the multi-agent + multi-model architecture when orchestrate=true
///   3. Respond in the user's language (ES/EN)
const HALCON_API_SYSTEM_PROMPT: &str = "\
You are Halcon, an autonomous AI assistant embedded in the Halcon multi-agent runtime.\n\
You have direct access to the host filesystem, shell execution, semantic web search, \
code analysis, git operations, and a registry of 60+ native tools.\n\
\n\
## Proactive Discovery (most important rule)\n\
When a file, directory, or project is NOT found at the expected path:\n\
  1. Use `bash` to search the filesystem:\n\
     find $HOME -name \"<name>\" -maxdepth 6 -type d 2>/dev/null | head -10\n\
  2. Use `glob` with `**/<name>` patterns from the home directory.\n\
  3. Use `fuzzy_find` for approximate / partial name matches.\n\
  4. Check common locations: ~/Documents, ~/Desktop, ~/Projects, ~/Code,\n\
     ~/Developer, ~/Github, ~/src, ~/workspace\n\
NEVER ask the user for a path before attempting at least one discovery tool call.\n\
\n\
## Multi-Step Reasoning\n\
Complex tasks require multiple sequential tool calls across rounds:\n\
  Discover → Read → Analyze → Synthesize\n\
Do not stop at the first obstacle — reason about alternatives and retry.\n\
\n\
## Architecture Awareness\n\
You operate inside the Halcon runtime with the following capabilities:\n\
  - Multi-agent: sub-agents can be spawned in parallel (orchestrate mode)\n\
  - Multi-model: multiple provider backends (deepseek, anthropic, openai, gemini, ollama)\n\
  - Multi-modal: file inspection, image analysis, code understanding\n\
  - Context pipeline: 5-tier L0-L4 context with semantic retrieval\n\
  - Tool ecosystem: filesystem, shell, git, web search, semantic search, code analysis\n\
\n\
## Tool Selection Guide\n\
| Task                  | Primary tools                                |\n\
|-----------------------|----------------------------------------------|\n\
| Locate file/project   | bash (find), glob, fuzzy_find                |\n\
| Read file content     | file_read, file_inspect                      |\n\
| Explore structure     | directory_tree, glob                         |\n\
| Search content        | grep, symbol_search                          |\n\
| Execute / run         | bash                                         |\n\
| Analyze codebase      | directory_tree → file_read → grep            |\n\
| Web / online research | web_search, native_search, web_fetch         |\n\
\n\
## Language\n\
Always respond in the same language the user writes in (Spanish if they write in Spanish).\n\
";

use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;
use halcon_core::traits::{
    ChatExecutionEvent, ChatExecutionInput, ChatExecutor as CoreChatExecutor,
};
use halcon_core::types::PermissionDecision;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::warn;
use uuid::Uuid;

use super::bridge_sink::BridgeSink;
use super::traits::AgentExecutor;
use super::traits::PermissionHandler;
use super::traits::StreamEmitter;
use super::types::AgentBridgeError;
use super::types::AgentStreamEvent;
use super::types::ChatTokenUsage;
use super::types::PermissionRequest;
use super::types::TurnContext;
use super::types::TurnResult;

pub struct AgentBridgeImpl {
    registry: Arc<halcon_providers::ProviderRegistry>,
    tool_registry: Arc<halcon_tools::ToolRegistry>,
}

impl AgentBridgeImpl {
    /// Create with empty registries (useful for tests with mock providers).
    pub fn new() -> Self {
        Self {
            registry: Arc::new(halcon_providers::ProviderRegistry::new()),
            tool_registry: Arc::new(halcon_tools::ToolRegistry::new()),
        }
    }

    /// Create with pre-built provider and tool registries (production path).
    pub fn with_registries(
        registry: Arc<halcon_providers::ProviderRegistry>,
        tool_registry: Arc<halcon_tools::ToolRegistry>,
    ) -> Self {
        Self { registry, tool_registry }
    }

    pub fn from_env() -> Self { Self::new() }
}

impl Default for AgentBridgeImpl {
    fn default() -> Self { Self::new() }
}

// AgentExecutor is ?Send because run_agent_loop holds EnteredSpan (!Send) across awaits.
#[async_trait(?Send)]
impl AgentExecutor for AgentBridgeImpl {
    async fn execute_turn(
        &self,
        context: TurnContext,
        emitter: Arc<dyn StreamEmitter>,
        permission_handler: Arc<dyn PermissionHandler>,
        cancellation: CancellationToken,
    ) -> Result<TurnResult, AgentBridgeError> {
        let started_at = Instant::now();
        let assistant_message_id = Uuid::new_v4();
        let session_id = context.session_id;
        let (perm_reply_tx, _perm_reply_rx) = mpsc::unbounded_channel::<PermissionDecision>();
        let ph_clone = permission_handler.clone();
        let perm_awaiter: crate::render::sink::PermissionAwaiter = Arc::new({
            let ph = ph_clone.clone();
            let sid = session_id;
            move |tool_name: &str, args: &serde_json::Value, risk_level: &str, deadline_secs: u64, reply_tx: tokio::sync::mpsc::UnboundedSender<PermissionDecision>| {
                let request = PermissionRequest {
                    request_id: Uuid::new_v4(),
                    session_id: sid,
                    tool_name: tool_name.to_string(),
                    risk_level: risk_level.to_string(),
                    args_preview: args.as_object().map(|o| o.iter().take(5).map(|(k, v)| (k.clone(), v.to_string())).collect()).unwrap_or_default(),
                    description: format!("Tool {} requires {} permission", tool_name, risk_level),
                    deadline_secs,
                };
                let ph2 = ph.clone();
                tokio::spawn(async move {
                    let decision = ph2.request_permission(request).await;
                    // B-extra: Log if the reply channel is closed (bridge_sink was dropped
                    // before the permission resolved).  Previously this was a silent discard.
                    if let Err(_dropped) = reply_tx.send(decision) {
                        tracing::warn!(
                            "permission decision dropped — bridge_sink was released before reply arrived"
                        );
                    }
                });
            }
        });
        let bridge_sink = BridgeSink::new(emitter.clone())
            .with_permission_awaiter(perm_awaiter, perm_reply_tx);
        if cancellation.is_cancelled() { emitter.emit(AgentStreamEvent::TurnFailed { error_code: "cancelled".to_string(), message: "Execution cancelled by user".to_string(), recoverable: true }); return Err(AgentBridgeError::CancelledByUser); }
        let result = tokio::select! {
            r = self.run_turn(context, bridge_sink, permission_handler) => r,
            _ = cancellation.cancelled() => {
                emitter.emit(AgentStreamEvent::TurnFailed {
                    error_code: "cancelled".to_string(),
                    message: "Execution cancelled by user".to_string(),
                    recoverable: true,
                });
                return Err(AgentBridgeError::CancelledByUser);
            }
        };
        match result {
            Ok(turn_result) => {
                let duration_ms = started_at.elapsed().as_millis() as u64;
                emitter.emit(AgentStreamEvent::TurnCompleted {
                    assistant_message_id,
                    stop_reason: turn_result.stop_reason.clone(),
                    usage: turn_result.usage.clone(),
                    total_duration_ms: duration_ms,
                });
                Ok(turn_result)
            }
            Err(e) => {
                emitter.emit(AgentStreamEvent::TurnFailed {
                    error_code: "execution_failed".to_string(),
                    message: e.to_string(),
                    recoverable: false,
                });
                Err(e)
            }
        }
    }
}

impl AgentBridgeImpl {
    async fn run_turn(
        &self,
        context: TurnContext,
        sink: BridgeSink,
        _permission_handler: Arc<dyn PermissionHandler>,
    ) -> Result<TurnResult, AgentBridgeError> {
        use halcon_core::types::{
            AgentLimits, ChatMessage, MessageContent, ModelRequest, OrchestratorConfig,
            Phase14Context, PlanningConfig, ResilienceConfig, Role, RoutingConfig,
            SecurityConfig, Session,
        };
        use halcon_core::EventSender;

        let started = std::time::Instant::now();

        // Resolve provider from the injected registry.
        let provider = self
            .registry
            .get(&context.provider)
            .cloned()
            .ok_or_else(|| {
                AgentBridgeError::ExecutionFailed(format!(
                    "Provider '{}' not found in registry",
                    context.provider
                ))
            })?;

        // Tool definitions from the shared registry.
        let tool_defs = self.tool_registry.tool_definitions();

        // Build session.
        let mut session = Session::new(
            context.model.clone(),
            context.provider.clone(),
            context.working_directory.clone(),
        );

        // Build event bus (audit events discarded; streaming goes via BridgeSink).
        let (event_tx, _event_rx): (EventSender, _) = halcon_core::event_bus(64);

        // Per-turn mutable infrastructure — cheap to create.
        let mut permissions =
            crate::repl::conversational_permission::ConversationalPermissionHandler::new(false);
        let limits = AgentLimits::default();
        let mut resilience =
            crate::repl::resilience::ResilienceManager::new(ResilienceConfig::default());
        let routing_config = RoutingConfig::default();
        let planning_config = PlanningConfig::default();
        let orchestrator_config = OrchestratorConfig {
            enabled: context.orchestrate,
            ..OrchestratorConfig::default()
        };
        let security_config = SecurityConfig::default();
        let speculator = crate::repl::tool_speculation::ToolSpeculator::new();

        // Build messages: prepend conversation history then append current user message.
        use super::types::TurnRole;
        let mut messages: Vec<ChatMessage> = context.history.iter().map(|m| {
            let role = match m.role {
                TurnRole::User => Role::User,
                TurnRole::Assistant => Role::Assistant,
                TurnRole::System => Role::System,
            };
            ChatMessage { role, content: MessageContent::Text(m.content.clone()) }
        }).collect();

        // Build the user message content — include image content blocks when present.
        let user_content = if context.media_attachments.is_empty() {
            MessageContent::Text(context.user_message.clone())
        } else {
            use halcon_core::types::{ContentBlock, ImageMediaType, ImageSource};
            let mut blocks: Vec<ContentBlock> = Vec::new();

            // Image attachments: send as vision blocks (provider converts to API format).
            for att in &context.media_attachments {
                if att.is_vision_image() {
                    let media_type = match att.content_type.as_str() {
                        "image/png"  => ImageMediaType::Png,
                        "image/webp" => ImageMediaType::Webp,
                        "image/gif"  => ImageMediaType::Gif,
                        _            => ImageMediaType::Jpeg, // jpeg default
                    };
                    blocks.push(ContentBlock::Image {
                        source: ImageSource::Base64 {
                            media_type,
                            data: att.data_base64.clone(),
                        },
                    });
                } else {
                    // Non-image files: decode and inline as a fenced code block.
                    let decoded_text = match base64_decode_to_text(&att.data_base64) {
                        Some(t) => t,
                        None => format!("[binary file: {} ({})]", att.filename, att.content_type),
                    };
                    let lang = lang_from_filename(&att.filename);
                    blocks.push(ContentBlock::Text {
                        text: format!("**Attached file: {}**\n```{}\n{}\n```", att.filename, lang, decoded_text),
                    });
                }
            }

            // Append the user's text message as the final block.
            if !context.user_message.is_empty() {
                blocks.push(ContentBlock::Text { text: context.user_message.clone() });
            }

            MessageContent::Blocks(blocks)
        };

        messages.push(ChatMessage {
            role: Role::User,
            content: user_content,
        });

        // Build model request.
        let request = ModelRequest {
            model: context.model.clone(),
            messages,
            tools: tool_defs,
            max_tokens: Some(8192),
            temperature: None,
            system: context.system_prompt.clone(),
            stream: true,
        };

        // Build AgentContext — BridgeSink satisfies RenderSink without ratatui.
        let ctx = crate::repl::agent::AgentContext {
            provider: &provider,
            session: &mut session,
            request: &request,
            tool_registry: &*self.tool_registry,
            permissions: &mut permissions,
            working_dir: &context.working_directory,
            event_tx: &event_tx,
            limits: &limits,
            trace_db: None,
            response_cache: None,
            resilience: &mut resilience,
            fallback_providers: &[],
            routing_config: &routing_config,
            compactor: None,
            planner: None,
            guardrails: &[],
            reflector: None,
            render_sink: &sink,
            replay_tool_executor: None,
            phase14: Phase14Context::default(),
            model_selector: None,
            registry: Some(&*self.registry),
            episode_id: Some(uuid::Uuid::new_v4()),
            planning_config: &planning_config,
            orchestrator_config: &orchestrator_config,
            tool_selection_enabled: false,
            task_bridge: None,
            context_metrics: None,
            context_manager: None,
            ctrl_rx: None,
            speculator: &speculator,
            security_config: &security_config,
            strategy_context: None,
            critic_provider: None,
            critic_model: None,
            plugin_registry: None,
            is_sub_agent: false,
        };

        let loop_result = crate::repl::agent::run_agent_loop(ctx)
            .await
            .map_err(|e| AgentBridgeError::ExecutionFailed(e.to_string()))?;

        let duration_ms = started.elapsed().as_millis() as u64;
        let stop_reason = format!("{:?}", loop_result.stop_condition).to_lowercase();

        Ok(TurnResult {
            assistant_text: loop_result.full_text,
            stop_reason,
            usage: ChatTokenUsage {
                input: loop_result.input_tokens,
                output: loop_result.output_tokens,
                thinking: 0,
            },
            duration_ms,
            tools_executed: Vec::new(),
            rounds: loop_result.rounds as u32,
            strategy_used: "direct_tool".to_string(),
        })
    }
}

/// Implementation of halcon-core's ChatExecutor trait.
///
/// Bridges the halcon-api ↔ agent pipeline without circular dependencies:
///   halcon-api → halcon-core (ChatExecutor trait)
///   halcon-cli → halcon-core (ChatExecutor impl)
///   No cycle: halcon-api does NOT import halcon-cli.
///
/// Thread model: `execute()` is `Send` (called via tokio::spawn from halcon-api).
/// `run_agent_loop` is `!Send` (tracing EnteredSpan across awaits). We bridge by
/// spawning a dedicated OS thread with a single-threaded Tokio runtime + LocalSet,
/// which allows running !Send futures safely.
#[async_trait]
impl CoreChatExecutor for AgentBridgeImpl {
    async fn execute(
        &self,
        input: ChatExecutionInput,
        event_tx: mpsc::UnboundedSender<ChatExecutionEvent>,
        mut cancel_rx: tokio::sync::watch::Receiver<bool>,
        mut perm_decision_rx: mpsc::UnboundedReceiver<(Uuid, bool)>,
    ) {
        let started_at = Instant::now();
        let assistant_message_id = Uuid::new_v4();
        let session_id = input.session_id;

        // Event translation: AgentStreamEvent → ChatExecutionEvent (on the main runtime).
        let (bridge_tx, mut bridge_rx) = mpsc::unbounded_channel::<AgentStreamEvent>();
        let et = event_tx.clone();
        let translator_task = tokio::spawn(async move {
            let mut sequence: u64 = 0;
            while let Some(event) = bridge_rx.recv().await {
                let opt = translate_stream_event(session_id, event, &mut sequence);
                if let Some(ev) = opt {
                    if et.send(ev).is_err() { break; }
                }
            }
        });

        // Wire cancel_rx → CancellationToken.
        let cancellation = CancellationToken::new();
        let ct = cancellation.clone();
        tokio::spawn(async move {
            loop {
                if cancel_rx.changed().await.is_err() { break; }
                if *cancel_rx.borrow() { ct.cancel(); break; }
            }
        });

        // Permission routing: perm_decision_rx → per-request oneshot senders.
        let perm_map: Arc<dashmap::DashMap<Uuid, tokio::sync::oneshot::Sender<PermissionDecision>>> =
            Arc::new(dashmap::DashMap::new());
        let perm_map_clone = perm_map.clone();
        tokio::spawn(async move {
            while let Some((request_id, approved)) = perm_decision_rx.recv().await {
                if let Some((_, tx)) = perm_map_clone.remove(&request_id) {
                    let decision = if approved { PermissionDecision::Allowed } else { PermissionDecision::Denied };
                    let _ = tx.send(decision);
                }
            }
        });

        // Build TurnContext (all Send).
        let turn_context = TurnContext {
            session_id,
            user_message: input.user_message,
            model: input.model,
            provider: input.provider,
            history: input.history.into_iter().map(|h| super::types::TurnMessage {
                role: match h.role.as_str() {
                    "assistant" => super::types::TurnRole::Assistant,
                    "system" => super::types::TurnRole::System,
                    _ => super::types::TurnRole::User,
                },
                content: h.content,
                created_at: chrono::Utc::now(),
            }).collect(),
            working_directory: input.working_directory,
            orchestrate: input.orchestrate,
            expert: input.expert,
            system_prompt: input.system_prompt,
            media_attachments: input.media_attachments,
        };

        // B1: Clone bridge_tx so the permission handler can emit PermissionExpired on timeout.
        // The main copy is consumed by ChannelEmitter; the clone is held by the handler.
        let perm_bridge_tx = bridge_tx.clone();
        // Build Send types for the thread.
        let bridge_emitter: Arc<dyn StreamEmitter> = Arc::new(ChannelEmitter::new(bridge_tx));
        let perm_handler: Arc<dyn PermissionHandler> = Arc::new(MapBasedPermissionHandler {
            perm_map,
            bridge_tx: perm_bridge_tx,
        });

        // Clone Arcs for the thread (cheap).
        let registry = Arc::clone(&self.registry);
        let tool_registry = Arc::clone(&self.tool_registry);

        // Spawn a dedicated OS thread with a single-threaded Tokio runtime.
        // run_agent_loop is !Send (tracing EnteredSpan), so it cannot run on
        // the multi-threaded main runtime's thread pool. LocalSet::block_on
        // allows !Send futures on a single thread.
        let (result_tx, result_rx) = tokio::sync::oneshot::channel::<Result<TurnResult, AgentBridgeError>>();
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("halcon: failed to build agent runtime");
            let bridge = AgentBridgeImpl { registry, tool_registry };
            let local = tokio::task::LocalSet::new();
            let result = local.block_on(&rt, async move {
                bridge.execute_turn(turn_context, bridge_emitter, perm_handler, cancellation).await
            });
            let _ = result_tx.send(result);
        });

        // Wait for the agent thread to complete.
        let result = result_rx.await
            .unwrap_or_else(|_| Err(AgentBridgeError::ExecutionFailed("agent thread died unexpectedly".to_string())));

        // Wait for the event translator to drain.
        let _ = translator_task.await;

        // Emit terminal event.
        let duration_ms = started_at.elapsed().as_millis() as u64;
        let final_event = match result {
            Ok(turn_result) => ChatExecutionEvent::Completed {
                assistant_message_id,
                stop_reason: turn_result.stop_reason,
                input_tokens: turn_result.usage.input,
                output_tokens: turn_result.usage.output,
                total_duration_ms: duration_ms,
            },
            Err(AgentBridgeError::CancelledByUser) => ChatExecutionEvent::Failed {
                error_code: "cancelled".to_string(),
                message: "Execution cancelled by user".to_string(),
                recoverable: true,
            },
            Err(e) => ChatExecutionEvent::Failed {
                error_code: "execution_failed".to_string(),
                message: e.to_string(),
                recoverable: false,
            },
        };
        let _ = event_tx.send(final_event);
    }
}

/// Translate an AgentStreamEvent to a ChatExecutionEvent.
fn translate_stream_event(
    session_id: Uuid,
    event: AgentStreamEvent,
    sequence: &mut u64,
) -> Option<ChatExecutionEvent> {
    let _ = session_id; // reserved for per-session filtering
    match event {
        AgentStreamEvent::OutputToken { token, .. } => {
            let ev = ChatExecutionEvent::Token { text: token, is_thinking: false, sequence_num: *sequence };
            *sequence += 1;
            Some(ev)
        }
        AgentStreamEvent::ThinkingToken { token } => {
            let ev = ChatExecutionEvent::Token { text: token, is_thinking: true, sequence_num: *sequence };
            *sequence += 1;
            Some(ev)
        }
        AgentStreamEvent::ThinkingProgressUpdate { chars_so_far, elapsed_secs } => {
            Some(ChatExecutionEvent::ThinkingProgress { chars_so_far, elapsed_secs })
        }
        AgentStreamEvent::ToolStarted { name, risk_level, .. } => {
            Some(ChatExecutionEvent::ToolStarted { name, risk_level })
        }
        AgentStreamEvent::ToolCompleted { name, duration_ms, success } => {
            Some(ChatExecutionEvent::ToolCompleted { name, duration_ms, success })
        }
        AgentStreamEvent::PermissionRequested { request_id, tool_name, risk_level, args_preview, description, deadline_secs } => {
            Some(ChatExecutionEvent::PermissionRequired { request_id, tool_name, risk_level, description, deadline_secs, args_preview })
        }
        AgentStreamEvent::PermissionExpired { request_id, .. } => {
            // B1: Forward expiry so the API layer can broadcast PermissionExpired to WS clients.
            Some(ChatExecutionEvent::PermissionExpired { request_id })
        }
        AgentStreamEvent::SubAgentStarted { sub_agent_id, task_description, wave, allowed_tools } => {
            Some(ChatExecutionEvent::SubAgentStarted { id: sub_agent_id, description: task_description, wave, allowed_tools })
        }
        AgentStreamEvent::SubAgentCompleted { sub_agent_id, success, summary, tools_used, duration_ms } => {
            Some(ChatExecutionEvent::SubAgentCompleted { id: sub_agent_id, success, summary, tools_used, duration_ms })
        }
        AgentStreamEvent::TurnCompleted { .. } | AgentStreamEvent::TurnFailed { .. } => None,
        _ => None,
    }
}

/// Permission handler backed by a request-id → oneshot map.
/// Used by AgentBridgeImpl::execute() to route HTTP decisions to the pipeline.
///
/// B1: Holds a bridge_tx so it can emit `PermissionExpired` when the deadline
/// fires.  This ensures every `PermissionRequested` event has a matching
/// `PermissionExpired` or `PermissionResolved` event — no orphaned modals.
struct MapBasedPermissionHandler {
    perm_map: Arc<dashmap::DashMap<Uuid, tokio::sync::oneshot::Sender<PermissionDecision>>>,
    bridge_tx: mpsc::UnboundedSender<AgentStreamEvent>,
}

#[async_trait]
impl PermissionHandler for MapBasedPermissionHandler {
    async fn request_permission(&self, request: PermissionRequest) -> PermissionDecision {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.perm_map.insert(request.request_id, tx);
        let started_at = std::time::Instant::now();
        // Wait up to deadline_secs for a decision; fail-closed on timeout.
        match tokio::time::timeout(
            std::time::Duration::from_secs(request.deadline_secs.max(5)),
            rx,
        )
        .await
        {
            Ok(Ok(decision)) => decision,
            timed_out_or_cancelled => {
                // Clean up the pending oneshot sender.
                self.perm_map.remove(&request.request_id);
                let deadline_elapsed_ms = started_at.elapsed().as_millis() as u64;
                // B1: Emit PermissionExpired so downstream clients can dismiss modals.
                let _ = self.bridge_tx.send(AgentStreamEvent::PermissionExpired {
                    request_id: request.request_id,
                    deadline_elapsed_ms,
                });
                if timed_out_or_cancelled.is_err() {
                    tracing::warn!(
                        request_id = %request.request_id,
                        tool = %request.tool_name,
                        elapsed_ms = deadline_elapsed_ms,
                        "permission request timed out — failing closed (Denied)"
                    );
                }
                PermissionDecision::Denied
            }
        }
    }
}

pub struct ChannelEmitter {
    tx: mpsc::UnboundedSender<AgentStreamEvent>,
}

impl ChannelEmitter {
    pub fn new(tx: mpsc::UnboundedSender<AgentStreamEvent>) -> Self { Self { tx } }
}

impl StreamEmitter for ChannelEmitter {
    fn emit(&self, event: AgentStreamEvent) {
        if self.tx.send(event).is_err() {
            warn!("StreamEmitter: downstream channel closed, event dropped");
        }
    }
    fn is_connected(&self) -> bool { !self.tx.is_closed() }
}

pub struct AutoApprovePermissionHandler;

#[async_trait]
impl PermissionHandler for AutoApprovePermissionHandler {
    async fn request_permission(&self, _request: PermissionRequest) -> PermissionDecision {
        PermissionDecision::Allowed
    }
}

pub struct AutoDenyPermissionHandler;

#[async_trait]
impl PermissionHandler for AutoDenyPermissionHandler {
    async fn request_permission(&self, _request: PermissionRequest) -> PermissionDecision {
        PermissionDecision::Denied
    }
}

// ── Multimodal helpers ────────────────────────────────────────────────────────

/// Try to decode a base64 string and return it as UTF-8 text.
/// Returns `None` if the bytes are not valid UTF-8 (binary file).
fn base64_decode_to_text(b64: &str) -> Option<String> {
    use std::io::Read;
    // Use the standard alphabet with padding tolerance.
    let bytes = {
        let mut buf = Vec::with_capacity(b64.len() * 3 / 4 + 4);
        let mut decoder = base64_reader(b64.as_bytes());
        decoder.read_to_end(&mut buf).ok()?;
        buf
    };
    String::from_utf8(bytes).ok()
}

/// Very minimal base64 decoder (avoids adding a new dep — uses the one already
/// available transitively).  Falls back to a simple stdlib approach.
fn base64_reader(input: &[u8]) -> impl std::io::Read + '_ {
    // We use a cursor + the ENGINE from the existing `base64` crate via a feature
    // already active in the workspace.  If the decode fails we return empty bytes.
    struct B64Cursor(std::io::Cursor<Vec<u8>>);
    impl std::io::Read for B64Cursor {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.0.read(buf)
        }
    }
    use base64::Engine as _;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(input)
        .unwrap_or_default();
    B64Cursor(std::io::Cursor::new(decoded))
}

/// Return a simple language tag for syntax-highlighted fencing based on filename extension.
fn lang_from_filename(filename: &str) -> &'static str {
    let ext = filename.rsplit('.').next().unwrap_or("").to_lowercase();
    match ext.as_str() {
        "rs" => "rust",
        "py" => "python",
        "js" | "mjs" => "javascript",
        "ts" => "typescript",
        "go" => "go",
        "java" => "java",
        "cpp" | "cc" | "cxx" => "cpp",
        "c" => "c",
        "rb" => "ruby",
        "sh" | "bash" => "bash",
        "json" => "json",
        "yaml" | "yml" => "yaml",
        "toml" => "toml",
        "md" | "markdown" => "markdown",
        "sql" => "sql",
        "html" | "htm" => "html",
        "css" => "css",
        "xml" => "xml",
        "csv" => "csv",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_bridge::types::{TurnMessage, TurnRole, PermissionDecisionKind};
    use chrono::Utc;

    /// Build an AgentBridgeImpl backed by EchoProvider (no real LLM calls).
    fn make_echo_bridge() -> AgentBridgeImpl {
        let mut registry = halcon_providers::ProviderRegistry::new();
        registry.register(Arc::new(halcon_providers::EchoProvider::new()));
        AgentBridgeImpl::with_registries(
            Arc::new(registry),
            Arc::new(halcon_tools::ToolRegistry::new()),
        )
    }

    fn make_context() -> TurnContext {
        TurnContext {
            session_id: Uuid::new_v4(),
            user_message: "test message".to_string(),
            model: "echo".to_string(),
            provider: "echo".to_string(),
            history: Vec::new(),
            working_directory: "/tmp".to_string(),
            orchestrate: false,
            expert: false,
            system_prompt: None,
            media_attachments: Vec::new(),
        }
    }

    #[tokio::test]
    async fn test_execute_turn_returns_result() {
        let bridge = make_echo_bridge();
        let (tx, _rx) = mpsc::unbounded_channel();
        let emitter = Arc::new(ChannelEmitter::new(tx));
        let perm_handler = Arc::new(AutoApprovePermissionHandler);
        let cancellation = CancellationToken::new();
        let result = bridge.execute_turn(make_context(), emitter, perm_handler, cancellation).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_execute_turn_cancelled() {
        let bridge = make_echo_bridge();
        let (tx, _rx) = mpsc::unbounded_channel();
        let emitter = Arc::new(ChannelEmitter::new(tx));
        let perm_handler = Arc::new(AutoApprovePermissionHandler);
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let result = bridge.execute_turn(make_context(), emitter, perm_handler, cancellation).await;
        assert!(matches!(result, Err(AgentBridgeError::CancelledByUser)));
    }

    #[tokio::test]
    async fn test_turn_completed_event_emitted() {
        let bridge = make_echo_bridge();
        let (tx, mut rx) = mpsc::unbounded_channel();
        let emitter = Arc::new(ChannelEmitter::new(tx));
        let perm_handler = Arc::new(AutoApprovePermissionHandler);
        let cancellation = CancellationToken::new();
        let _ = bridge.execute_turn(make_context(), emitter, perm_handler, cancellation).await;
        let mut found_completed = false;
        while let Ok(event) = rx.try_recv() {
            if matches!(event, AgentStreamEvent::TurnCompleted { .. }) {
                found_completed = true;
            }
        }
        assert!(found_completed, "TurnCompleted event must be emitted");
    }

    #[test]
    fn test_permission_decision_kind() {
        assert_ne!(PermissionDecisionKind::Approved, PermissionDecisionKind::Rejected);
        assert_ne!(PermissionDecisionKind::Approved, PermissionDecisionKind::TimedOut);
    }

    #[test]
    fn test_chat_token_usage_total() {
        let usage = ChatTokenUsage { input: 100, output: 200, thinking: 50 };
        assert_eq!(usage.total(), 350);
    }

    #[test]
    fn test_channel_emitter_is_connected() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let emitter = ChannelEmitter::new(tx);
        assert!(emitter.is_connected());
    }

    #[test]
    fn test_channel_emitter_disconnected_when_rx_dropped() {
        let (tx, rx) = mpsc::unbounded_channel::<AgentStreamEvent>();
        let emitter = ChannelEmitter::new(tx);
        drop(rx);
        assert!(!emitter.is_connected());
    }

    #[test]
    fn test_turn_message_has_role() {
        let msg = TurnMessage {
            role: TurnRole::User,
            content: "hello".to_string(),
            created_at: Utc::now(),
        };
        assert_eq!(msg.role, TurnRole::User);
    }
}
