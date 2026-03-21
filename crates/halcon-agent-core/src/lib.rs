// Allow deprecated rand methods and unused test imports — these are caught
// only on CI's newer Rust version (1.85) and will be cleaned up progressively.
#![allow(deprecated, unused_imports, unused_variables, dead_code)]

//! # halcon-agent-core — SOTA Goal-Driven Execution Model (GDEM)
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────────┐
//! │                HALCON GDEM — Layer Stack                        │
//! │                                                                 │
//! │  L0  GoalSpecificationEngine  parse intent → VerifiableCriteria│
//! │  L1  AdaptivePlanner          tree-of-thoughts branching plan   │
//! │  L2  SemanticToolRouter       embedding-based tool selection    │
//! │  L3  SandboxedExecutor        (halcon-sandbox crate)            │
//! │  L4  StepVerifier             in-loop goal criterion check      │
//! │  L5  InLoopCritic             per-round alignment scoring       │
//! │  L6  FormalAgentFSM           typed state-machine (compile-safe)│
//! │  L7  VectorMemory             HNSW episodic + long-term memory  │
//! │  L8  UCB1StrategyLearner      cross-session strategy learning   │
//! │  L9  MultiAgentOrchestrator   DAG-based task decomposition      │
//! └─────────────────────────────────────────────────────────────────┘
//! ```
//!
//! ## Key Design Invariants
//!
//! 1. **Goal-First Termination**: the agent loop exits when
//!    `GoalVerificationEngine::evaluate() >= threshold`, NOT when tools stagnate.
//! 2. **In-Loop Critic**: `InLoopCritic` runs after every tool batch and injects
//!    structured feedback before the next round — never post-hoc.
//! 3. **Zero Hardcoded Tool Mapping**: `SemanticToolRouter` selects tools by
//!    embedding cosine similarity; no static keyword tables exist.
//! 4. **Typed FSM**: all agent state transitions are validated at compile time
//!    via Rust's type-state pattern; invalid transitions are type errors.
//! 5. **Sandbox-First Execution**: bash and all shell tools run inside
//!    `halcon-sandbox`; no direct host shell access from agent code.

pub mod adversarial_simulation_tests;
pub mod confidence_hysteresis;
pub mod critic;
pub mod execution_budget;
pub mod failure_injection;
pub mod fsm;
pub mod fsm_formal_model;
pub mod goal;
pub mod info_theory_metrics;
pub mod invariant_coverage;
pub mod invariants;
pub mod long_horizon_tests;
pub mod loop_driver;
pub mod memory;
pub mod metrics;
pub mod orchestrator;
pub mod oscillation_metric;
pub mod planner;
pub mod regret_analysis;
pub mod replay_certification;
pub mod router;
pub mod stability_analysis;
pub mod strategy;
pub mod telemetry;
pub mod verifier;

// Re-export the primary public API surface.
pub use critic::{CriticConfig, CriticSignal, InLoopCritic};
pub use fsm::{AgentFsm, AgentState, FsmError};
pub use goal::{
    ConfidenceScore, GoalSpec, GoalVerificationEngine, VerifiableCriterion, VerificationResult,
};
pub use loop_driver::{run_gdem_loop, GdemConfig, GdemContext, GdemResult};
pub use memory::{Episode, MemoryConfig, VectorMemory};
pub use orchestrator::{DagOrchestrator, OrchestratorConfig};
pub use planner::{AdaptivePlanner, PlanBranch, PlanTree, PlannerConfig};
pub use router::{RouterConfig, SemanticToolRouter, ToolCandidate};
pub use strategy::{StrategyLearner, StrategyLearnerConfig};
pub use telemetry::{InvocationRecord, ToolTelemetry};
pub use verifier::{StepVerifier, VerifierConfig};
