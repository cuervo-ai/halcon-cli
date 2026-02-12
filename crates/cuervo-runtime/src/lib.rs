//! Universal multi-agent orchestration runtime for Cuervo CLI.
//!
//! Provides the abstraction layer, transport, registry, federation,
//! executor, plugin system, and protocol bridges for orchestrating
//! any external system as a first-class agent.

pub mod agent;
pub mod capability;
pub mod error;

// Phase R-1
pub mod transport;

// Phase R-2
pub mod health;
pub mod registry;

// Phase R-3
pub mod federation;

// Phase R-4
pub mod executor;

// Phase R-5
pub mod plugin;

// Phase R-6
pub mod bridges;

// Phase R-7
pub mod runtime;

pub use agent::*;
pub use capability::CapabilityIndex;
pub use error::{Result, RuntimeError};

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke_test() {
        assert!(!version().is_empty());
    }
}
