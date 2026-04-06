use std::sync::Arc;

use anyhow::Result;
use halcon_core::error::HalconError;
use halcon_core::traits::ModelProvider;
use halcon_core::types::{DomainEvent, EventPayload, ModelChunk, ModelRequest, RoutingConfig};
use halcon_core::EventSender;

use super::super::resilience::{PreInvokeDecision, ResilienceManager};
use super::super::speculative::SpeculativeInvoker;
use crate::render::sink::RenderSink;

/// Result of an invocation attempt through the routing + resilience chain.
pub(super) struct InvokeAttempt {
    pub stream: futures::stream::BoxStream<'static, Result<ModelChunk, HalconError>>,
    pub provider_name: String,
    pub is_fallback: bool,
    #[allow(dead_code)]
    pub permit: Option<super::super::backpressure::InvokePermit>,
}

/// Invoke a provider with resilience gating and speculative/failover routing.
///
/// When resilience is enabled, pre-filters healthy providers via the ResilienceManager,
/// then delegates to SpeculativeInvoker for retry + fallback / speculative racing.
/// When resilience is disabled, delegates directly to SpeculativeInvoker.
pub(super) async fn invoke_with_fallback(
    primary: &Arc<dyn ModelProvider>,
    request: &ModelRequest,
    fallback_providers: &[(String, Arc<dyn ModelProvider>)],
    resilience: &mut ResilienceManager,
    routing_config: &RoutingConfig,
    event_tx: &EventSender,
) -> Result<InvokeAttempt> {
    let invoker = SpeculativeInvoker::new(routing_config);

    // If resilience is disabled, delegate directly to the speculative invoker.
    if !resilience.is_enabled() {
        let result = invoker.invoke(primary, request, fallback_providers).await?;
        return Ok(InvokeAttempt {
            stream: result.stream,
            provider_name: result.provider_name,
            is_fallback: result.is_fallback,
            permit: None,
        });
    }

    // Pre-filter: collect healthy providers via resilience pre_invoke.
    let mut healthy_primary: Option<(
        Arc<dyn ModelProvider>,
        super::super::backpressure::InvokePermit,
    )> = None;
    let mut healthy_fallbacks: Vec<(String, Arc<dyn ModelProvider>)> = Vec::new();

    // Check primary.
    match resilience.pre_invoke(primary.name()).await {
        PreInvokeDecision::Proceed { permit } => {
            healthy_primary = Some((Arc::clone(primary), permit));
        }
        PreInvokeDecision::Fallback { reason } => {
            tracing::info!(
                provider = primary.name(),
                reason = %reason,
                "Primary provider rejected by resilience"
            );
        }
    }

    // Check fallbacks (permits are advisory for fallbacks — drop after check).
    for (name, fb_provider) in fallback_providers {
        match resilience.pre_invoke(name).await {
            PreInvokeDecision::Proceed { permit: _permit } => {
                healthy_fallbacks.push((name.clone(), Arc::clone(fb_provider)));
            }
            PreInvokeDecision::Fallback { reason } => {
                tracing::debug!(
                    provider = %name,
                    reason = %reason,
                    "Fallback provider rejected by resilience"
                );
            }
        }
    }

    // If no healthy providers at all, bail.
    if healthy_primary.is_none() && healthy_fallbacks.is_empty() {
        anyhow::bail!(
            "All providers unavailable (primary '{}' + {} fallbacks)",
            primary.name(),
            fallback_providers.len()
        );
    }

    // Determine the effective primary and fallbacks for the invoker.
    let (effective_primary, permit, promoted_name) = if let Some((p, permit)) = healthy_primary {
        (p, Some(permit), None)
    } else {
        // Primary is unhealthy — promote first healthy fallback to primary.
        let (name, first_fb) = healthy_fallbacks.remove(0);
        tracing::info!(provider = %name, "Promoting fallback to primary (original primary unhealthy)");
        halcon_core::emit_event(
            event_tx,
            DomainEvent::new(EventPayload::ProviderFallback {
                from_provider: primary.name().to_string(),
                to_provider: name.clone(),
                reason: "primary unhealthy".to_string(),
            }),
        );
        (first_fb, None, Some(name))
    };

    // Delegate to speculative invoker.
    match invoker
        .invoke(&effective_primary, request, &healthy_fallbacks)
        .await
    {
        Ok(result) => {
            // If we promoted a fallback, use the logical name and mark as fallback.
            let (provider_name, is_fallback) = if let Some(name) = promoted_name {
                (name, true)
            } else {
                (result.provider_name, result.is_fallback)
            };
            Ok(InvokeAttempt {
                stream: result.stream,
                provider_name,
                is_fallback,
                permit,
            })
        }
        Err(e) => {
            // Record failure on the effective primary.
            resilience.record_failure(effective_primary.name()).await;
            tracing::warn!(
                provider = effective_primary.name(),
                "Primary/promoted provider failed: {e}, trying remaining fallbacks"
            );

            // Retry chain: try each remaining healthy fallback sequentially.
            // Each fallback gets a request with a model it actually supports.
            for (idx, (fb_name, fb_provider)) in healthy_fallbacks.iter().enumerate() {
                let fb_request = if fb_provider
                    .supported_models()
                    .iter()
                    .any(|m| m.id == request.model)
                {
                    request.clone()
                } else if let Some(default) = fb_provider.supported_models().first() {
                    tracing::info!(
                        provider = %fb_name,
                        original_model = %request.model,
                        fallback_model = %default.id,
                        "Adjusting model for fallback provider"
                    );
                    ModelRequest {
                        model: default.id.clone(),
                        ..request.clone()
                    }
                } else {
                    request.clone()
                };
                match fb_provider.invoke(&fb_request).await {
                    Ok(stream) => {
                        halcon_core::emit_event(
                            event_tx,
                            DomainEvent::new(EventPayload::ProviderFallback {
                                from_provider: effective_primary.name().to_string(),
                                to_provider: fb_name.clone(),
                                reason: format!("fallback #{}", idx + 1),
                            }),
                        );
                        return Ok(InvokeAttempt {
                            stream,
                            provider_name: fb_name.clone(),
                            is_fallback: true,
                            permit: None,
                        });
                    }
                    Err(fb_err) => {
                        tracing::warn!(provider = %fb_name, "Fallback provider failed: {fb_err}");
                        resilience.record_failure(fb_name).await;
                    }
                }
            }

            // All fallbacks exhausted.
            anyhow::bail!(
                "All providers failed (primary + {} fallbacks): {e}",
                healthy_fallbacks.len()
            )
        }
    }
}

/// Check the control channel for pause/step/cancel events.
///
/// Non-blocking: returns immediately if no events pending.
/// On Pause: blocks until Resume, Step, or Cancel is received.
/// Returns the action the agent loop should take.
///
/// All ControlEvent variants are handled explicitly — no silent ignores.
/// ApproveAction/RejectAction are permission responses handled by the
/// dedicated permission channel in TUI mode; they are no-ops here.
#[cfg(feature = "tui")]
pub(crate) async fn check_control(
    ctrl_rx: &mut tokio::sync::mpsc::UnboundedReceiver<crate::tui::events::ControlEvent>,
    sink: &dyn RenderSink,
) -> super::ControlAction {
    use super::ControlAction;
    use crate::tui::events::ControlEvent;
    match ctrl_rx.try_recv() {
        Ok(ControlEvent::Pause) => {
            sink.info("  [paused] Press Space to resume, N to step");
            // Block until Resume, Step, or Cancel.
            loop {
                match ctrl_rx.recv().await {
                    Some(ControlEvent::Resume) => return ControlAction::Continue,
                    Some(ControlEvent::Step) => return ControlAction::StepOnce,
                    Some(ControlEvent::CancelAgent) => return ControlAction::Cancel,
                    None => return ControlAction::Cancel, // Channel closed.
                    // Permission events are handled by the dedicated permission
                    // channel, not the control channel. Log and continue waiting.
                    Some(ControlEvent::Pause) => {
                        // Already paused — no-op.
                    }
                    Some(ControlEvent::ApproveAction | ControlEvent::RejectAction) => {
                        tracing::debug!("Permission event received on control channel while paused (handled by permission channel)");
                    }
                    Some(ControlEvent::RequestContextServers) => {
                        // Context server requests are handled by the repl loop, not the agent loop.
                        tracing::trace!(
                            "RequestContextServers received while paused (handled by repl loop)"
                        );
                    }
                    Some(ControlEvent::ResumeSession(_)) => {
                        // Session resume is handled by the repl loop, not the agent loop.
                        tracing::trace!(
                            "ResumeSession received while paused (handled by repl loop)"
                        );
                    }
                    Some(ControlEvent::SwitchModel { .. }) => {
                        // Model switch is handled by the repl loop, not the agent loop.
                        tracing::trace!("SwitchModel received while paused (handled by repl loop)");
                    }
                }
            }
        }
        Ok(ControlEvent::CancelAgent) => ControlAction::Cancel,
        Ok(ControlEvent::Step) => ControlAction::StepOnce,
        Ok(ControlEvent::Resume) => {
            // Resume without prior pause — treat as continue.
            ControlAction::Continue
        }
        Ok(ControlEvent::ApproveAction | ControlEvent::RejectAction) => {
            // Permission events are handled by the dedicated permission channel.
            tracing::debug!(
                "Permission event received on control channel (handled by permission channel)"
            );
            ControlAction::Continue
        }
        Ok(ControlEvent::RequestContextServers) => {
            // Context server requests are handled by the repl loop, not the agent loop.
            tracing::trace!("RequestContextServers received in agent loop (handled by repl loop)");
            ControlAction::Continue
        }
        Ok(ControlEvent::ResumeSession(_)) => {
            // Session resume is handled by the repl loop, not the agent loop.
            tracing::trace!("ResumeSession received in agent loop (handled by repl loop)");
            ControlAction::Continue
        }
        Ok(ControlEvent::SwitchModel { .. }) => {
            // Model switch is handled by the repl loop, not the agent loop.
            tracing::trace!("SwitchModel received in agent loop (handled by repl loop)");
            ControlAction::Continue
        }
        Err(_) => ControlAction::Continue, // No events pending.
    }
}

/// Classic REPL check_control — handles ClassicCancelSignal::Cancel from Ctrl-C channel.
///
/// DECISION (BUG-001 / GAP-CTRLC): ControlReceiver (non-TUI) was changed from `()` to
/// `Receiver<ClassicCancelSignal>` to wire Ctrl-C support. This stub must match the type.
/// A pending Cancel signal maps to ControlAction::Cancel; no signal → Continue.
#[cfg(not(feature = "tui"))]
pub(crate) async fn check_control(
    ctrl_rx: &mut tokio::sync::mpsc::Receiver<super::super::agent_types::ClassicCancelSignal>,
    _sink: &dyn RenderSink,
) -> super::ControlAction {
    use super::super::agent_types::ClassicCancelSignal;
    match ctrl_rx.try_recv() {
        Ok(ClassicCancelSignal::Cancel) => super::ControlAction::Cancel,
        Err(_) => super::ControlAction::Continue,
    }
}
