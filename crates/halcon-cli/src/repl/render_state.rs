//! Render and UI state bundle.
//!
//! Phase 3.1: Groups all presentation layer configuration (sinks, modes, flags)
//! into a single cohesive unit. 5 fields from original Repl.

use std::sync::Arc;

use crate::render::ci_sink::CiSink;

/// Render and UI configuration state for the REPL.
///
/// Controls presentation layer behavior:
/// - Banner display
/// - Expert mode verbosity
/// - Output format (classic vs NDJSON)
/// - CI sink for JSON output
/// - Model selection feedback
pub struct ReplRenderState {
    /// When true, suppress startup banner.
    pub no_banner: bool,

    /// Expert mode: show full agent feedback (model selection, caching, etc.).
    pub expert_mode: bool,

    /// US-output-format (PASO 2-A): when true, use CiSink (NDJSON) instead of ClassicSink.
    /// Set from --output-format json on the CLI.
    pub use_ci_sink: bool,

    /// Shared CiSink instance for session_end emission after loop completes.
    /// Only populated when use_ci_sink is true.
    pub ci_sink: Option<Arc<CiSink>>,

    /// When true, the user explicitly set `--model` on the CLI, so model selection is bypassed.
    /// Affects UI feedback - when true, model selection reasoning is not shown.
    pub explicit_model: bool,
}

impl ReplRenderState {
    /// Construct render state with all components.
    pub fn new(
        no_banner: bool,
        expert_mode: bool,
        use_ci_sink: bool,
        ci_sink: Option<Arc<CiSink>>,
        explicit_model: bool,
    ) -> Self {
        Self {
            no_banner,
            expert_mode,
            use_ci_sink,
            ci_sink,
            explicit_model,
        }
    }

    /// Check if using JSON output mode (vs classic terminal output).
    pub fn is_json_mode(&self) -> bool {
        self.use_ci_sink
    }

    /// Check if should show model selection reasoning.
    pub fn should_show_model_selection(&self) -> bool {
        !self.explicit_model && self.expert_mode
    }
}

impl Default for ReplRenderState {
    fn default() -> Self {
        Self {
            no_banner: false,
            expert_mode: false,
            use_ci_sink: false,
            ci_sink: None,
            explicit_model: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_state_default_is_classic_terminal_mode() {
        let render = ReplRenderState::default();

        assert!(!render.no_banner, "banner shown by default");
        assert!(!render.expert_mode, "expert mode off by default");
        assert!(!render.is_json_mode(), "classic terminal output by default");
        assert!(!render.explicit_model, "no explicit model by default");
    }

    #[test]
    fn render_state_json_mode_detection() {
        let json_render = ReplRenderState::new(false, false, true, None, false);
        assert!(json_render.is_json_mode());

        let terminal_render = ReplRenderState::new(false, false, false, None, false);
        assert!(!terminal_render.is_json_mode());
    }

    #[test]
    fn render_state_model_selection_feedback() {
        // Expert mode + no explicit model = show selection
        let show = ReplRenderState::new(false, true, false, None, false);
        assert!(show.should_show_model_selection());

        // Explicit model set = hide selection (even in expert mode)
        let hide = ReplRenderState::new(false, true, false, None, true);
        assert!(!hide.should_show_model_selection());

        // Not expert mode = hide selection
        let hide2 = ReplRenderState::new(false, false, false, None, false);
        assert!(!hide2.should_show_model_selection());
    }
}
