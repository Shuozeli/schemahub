//! The compiler registry: `format_id` → `Arc<dyn Compiler>`
//! (crate-structure.md §3.3).

use std::collections::HashMap;
use std::sync::Arc;

use schemahub_types::Compiler;

/// Maps a format id ("protobuf" | "flatbuffers" | "openapi") to its compiler.
/// Compilers are injected at startup by `schemahub-server` (the composition root).
#[derive(Default, Clone)]
pub struct CompilerRegistry {
    compilers: HashMap<String, Arc<dyn Compiler>>,
}

impl CompilerRegistry {
    pub fn new() -> Self {
        Self {
            compilers: HashMap::new(),
        }
    }

    /// Register a compiler under its `format_id`.
    pub fn register(&mut self, compiler: Arc<dyn Compiler>) {
        self.compilers
            .insert(compiler.format_id().to_string(), compiler);
    }

    /// Look up the compiler for a format id.
    pub fn get(&self, format_id: &str) -> Option<&Arc<dyn Compiler>> {
        self.compilers.get(format_id)
    }

    /// All registered format ids.
    pub fn format_ids(&self) -> impl Iterator<Item = &str> {
        self.compilers.keys().map(String::as_str)
    }
}
