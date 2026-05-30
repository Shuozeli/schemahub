//! Server configuration (crate-structure.md §3.6): db backend + path, listen
//! address, and bootstrap per-repo compatibility config. Loaded from
//! `schemahub.toml` (optional) and overridable by CLI flags.

use std::collections::HashMap;

use schemahub_core::{RepoConfig, RepoConfigStore};
use schemahub_types::CompatibilityDirection;
use serde::Deserialize;

/// Top-level server config, deserialized from `schemahub.toml`.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub storage: StorageConfig,
    #[serde(default)]
    pub listen: ListenConfig,
    /// Per-repo compatibility/protection config, keyed by "project/repo".
    #[serde(default)]
    pub repos: HashMap<String, RepoSection>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StorageConfig {
    /// Backend id: currently only "redb" is wired.
    #[serde(default = "default_backend")]
    pub backend: String,
    /// Path to the redb database file.
    #[serde(default = "default_db_path")]
    pub path: String,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            backend: default_backend(),
            path: default_db_path(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ListenConfig {
    /// Listen address, e.g. "0.0.0.0:50051". Overridden by `TAILSCALE_IP` env
    /// (user infra convention) when no explicit address is given.
    #[serde(default = "default_addr")]
    pub addr: String,
}

impl Default for ListenConfig {
    fn default() -> Self {
        Self {
            addr: default_addr(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct RepoSection {
    #[serde(default)]
    pub default_bookmark: Option<String>,
    /// "backward" | "forward" | "full" | "disabled".
    #[serde(default)]
    pub compatibility: Option<String>,
    #[serde(default)]
    pub protected_bookmarks: Option<Vec<String>>,
}

fn default_backend() -> String {
    "redb".to_string()
}
fn default_db_path() -> String {
    "schemahub.db".to_string()
}
fn default_addr() -> String {
    "0.0.0.0:50051".to_string()
}

impl Config {
    /// Load from a TOML file if it exists, else defaults.
    pub fn load(path: &str) -> anyhow::Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(s) => Ok(toml::from_str(&s)?),
            Err(_) => Ok(Self::default()),
        }
    }

    /// Build the [`RepoConfigStore`] the core consumes from the `[repos.*]`
    /// sections.
    pub fn repo_config_store(&self) -> RepoConfigStore {
        let mut store = RepoConfigStore::new();
        for (key, section) in &self.repos {
            let Some((project, repo)) = key.split_once('/') else {
                continue;
            };
            let mut cfg = RepoConfig::default();
            if let Some(b) = &section.default_bookmark {
                cfg.default_bookmark = b.clone();
            }
            if let Some(c) = &section.compatibility {
                cfg.compatibility_direction = parse_direction(c);
            }
            if let Some(p) = &section.protected_bookmarks {
                cfg.protected_bookmarks = p.clone();
            }
            store.set(project, repo, cfg);
        }
        store
    }
}

fn parse_direction(s: &str) -> CompatibilityDirection {
    match s.to_lowercase().as_str() {
        "backward" => CompatibilityDirection::Backward,
        "forward" => CompatibilityDirection::Forward,
        "disabled" | "none" => CompatibilityDirection::Disabled,
        _ => CompatibilityDirection::Full,
    }
}
