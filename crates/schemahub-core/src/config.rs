//! Per-repo configuration: protected bookmarks + compatibility direction
//! (design.md §7). Ported from v1 `repo_config.rs`, but held in-memory by the
//! core rather than persisted through a storage backend — `schemahub-server`
//! loads it from `schemahub.toml` and seeds it at startup. The VCS layer never
//! sees this config; it is purely the core's compatibility-gating policy.

use std::collections::HashMap;

use schemahub_types::{CompatibilityDirection, CompatibilityRules};

/// Configuration for a single repository's protected bookmarks and compat policy.
#[derive(Clone, Debug)]
pub struct RepoConfig {
    /// The repo's default/primary bookmark name (informational; used for log/diff
    /// defaults by the server).
    pub default_bookmark: String,
    /// The compatibility direction enforced on protected bookmarks.
    pub compatibility_direction: CompatibilityDirection,
    /// Bookmark names (or `prefix/*` globs) that are protected: the only places
    /// compatibility is enforced (design.md §7).
    pub protected_bookmarks: Vec<String>,
}

impl Default for RepoConfig {
    fn default() -> Self {
        Self {
            default_bookmark: "main".to_string(),
            compatibility_direction: CompatibilityDirection::Full,
            protected_bookmarks: vec!["main".to_string()],
        }
    }
}

impl RepoConfig {
    /// The [`CompatibilityRules`] this repo enforces (direction + the
    /// `Disabled` short-circuit).
    pub fn compat_rules(&self) -> CompatibilityRules {
        CompatibilityRules {
            direction: self.compatibility_direction,
            disabled: matches!(
                self.compatibility_direction,
                CompatibilityDirection::Disabled
            ),
        }
    }
}

/// In-memory map of `(project, repo)` → [`RepoConfig`]. Missing entries fall back
/// to [`RepoConfig::default`] (protect `main`, FULL compatibility).
#[derive(Clone, Debug, Default)]
pub struct RepoConfigStore {
    configs: HashMap<(String, String), RepoConfig>,
}

impl RepoConfigStore {
    pub fn new() -> Self {
        Self {
            configs: HashMap::new(),
        }
    }

    /// Register a repo's config (overwrites any existing entry).
    pub fn set(&mut self, project: impl Into<String>, repo: impl Into<String>, config: RepoConfig) {
        self.configs.insert((project.into(), repo.into()), config);
    }

    /// The config for a repo, or the default if none was registered.
    pub fn get(&self, project: &str, repo: &str) -> RepoConfig {
        self.configs
            .get(&(project.to_string(), repo.to_string()))
            .cloned()
            .unwrap_or_default()
    }
}
