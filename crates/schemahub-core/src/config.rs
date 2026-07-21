//! Per-repo configuration: protected bookmarks + compatibility direction
//! (design.md §7). The server seeds startup TOML into durable repository
//! resources; runtime reads prefer that redb/PostgreSQL record. The JJ layer
//! never interprets this config; it remains core-owned publication policy.

use std::collections::HashMap;

use schemahub_types::{CompatibilityDirection, CompatibilityRules};

/// Review/publication policy for one repository.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReviewPolicy {
    /// Number of distinct maintainer approvals required before Apply.
    pub required_approvals: u32,
    /// When true, direct SchemaService writes are disabled and publication must
    /// flow through a durable ChangeRecord.
    pub require_change_record: bool,
}

/// Artifact kinds exposed by the immutable serving plane.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ServingPolicy {
    pub source: bool,
    pub descriptors: bool,
    pub generated_code: bool,
}

impl Default for ServingPolicy {
    fn default() -> Self {
        Self {
            source: true,
            descriptors: true,
            generated_code: true,
        }
    }
}

/// Configuration for a single repository's protected bookmarks and compat policy.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RepoConfig {
    /// The repo's default/primary bookmark name (informational; used for log/diff
    /// defaults by the server).
    pub default_bookmark: String,
    /// The compatibility direction enforced on protected bookmarks.
    pub compatibility_direction: CompatibilityDirection,
    /// Bookmark names (or `prefix/*` globs) that are protected: the only places
    /// compatibility is enforced (design.md §7).
    pub protected_bookmarks: Vec<String>,
    /// Review and publication workflow policy.
    #[serde(default)]
    pub review_policy: ReviewPolicy,
    /// Artifact exposure policy for immutable serving.
    #[serde(default)]
    pub serving_policy: ServingPolicy,
}

impl Default for RepoConfig {
    fn default() -> Self {
        Self {
            default_bookmark: "main".to_string(),
            compatibility_direction: CompatibilityDirection::Full,
            protected_bookmarks: vec!["main".to_string()],
            review_policy: ReviewPolicy::default(),
            serving_policy: ServingPolicy::default(),
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

    /// Stable snapshot used to seed the durable repository registry from
    /// startup configuration. Runtime records are never overwritten by a
    /// subsequent bootstrap.
    pub fn entries(&self) -> Vec<(String, String, RepoConfig)> {
        let mut entries: Vec<_> = self
            .configs
            .iter()
            .map(|((project, repo), config)| (project.clone(), repo.clone(), config.clone()))
            .collect();
        entries.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
        entries
    }
}
