use std::collections::HashMap;
use std::sync::Arc;

use schemahub_types::FormatPlugin;

/// Registry of FormatPlugins keyed by their format_id.
pub struct PluginRegistry {
    plugins: HashMap<String, Arc<dyn FormatPlugin>>,
}

impl PluginRegistry {
    pub fn new() -> Self {
        Self {
            plugins: HashMap::new(),
        }
    }

    /// Register a plugin. If a plugin with the same format_id already exists,
    /// it is replaced.
    pub fn register(&mut self, plugin: Arc<dyn FormatPlugin>) {
        self.plugins.insert(plugin.format_id().to_string(), plugin);
    }

    /// Look up a plugin by format_id.
    pub fn get(&self, format_id: &str) -> Option<&Arc<dyn FormatPlugin>> {
        self.plugins.get(format_id)
    }
}

impl Default for PluginRegistry {
    fn default() -> Self {
        Self::new()
    }
}
