use std::collections::HashMap;
use std::sync::Arc;

use cuervo_core::traits::ModelProvider;

/// Registry of available model providers.
///
/// Holds provider instances and routes requests to the appropriate one.
pub struct ProviderRegistry {
    providers: HashMap<String, Arc<dyn ModelProvider>>,
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self {
            providers: HashMap::new(),
        }
    }

    pub fn register(&mut self, provider: Arc<dyn ModelProvider>) {
        self.providers.insert(provider.name().to_string(), provider);
    }

    pub fn get(&self, name: &str) -> Option<&Arc<dyn ModelProvider>> {
        self.providers.get(name)
    }

    pub fn list(&self) -> Vec<&str> {
        self.providers.keys().map(|s| s.as_str()).collect()
    }
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EchoProvider;

    #[test]
    fn register_and_retrieve() {
        let mut registry = ProviderRegistry::new();
        let provider: Arc<dyn ModelProvider> = Arc::new(EchoProvider::new());
        registry.register(provider);

        let retrieved = registry.get("echo");
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().name(), "echo");
    }

    #[test]
    fn get_unknown_returns_none() {
        let registry = ProviderRegistry::new();
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn list_returns_all() {
        let mut registry = ProviderRegistry::new();
        let echo: Arc<dyn ModelProvider> = Arc::new(EchoProvider::new());
        registry.register(echo);

        let names = registry.list();
        assert_eq!(names.len(), 1);
        assert!(names.contains(&"echo"));
    }

    #[test]
    fn empty_registry_list() {
        let registry = ProviderRegistry::new();
        assert!(registry.list().is_empty());
    }

    #[test]
    fn default_creates_empty() {
        let registry = ProviderRegistry::default();
        assert!(registry.list().is_empty());
        assert!(registry.get("anything").is_none());
    }
}
