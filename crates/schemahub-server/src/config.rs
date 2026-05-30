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

/// Storage backend config. `backend` selects between the embedded redb default
/// and a Postgres deployment; the relevant subset of fields is honored per
/// backend (`path` for redb, `url` for postgres).
#[derive(Debug, Clone, Deserialize)]
pub struct StorageConfig {
    /// Backend id: `"redb"` (default, embedded) or `"postgres"` (server-mode;
    /// requires the `postgres` feature on `schemahub-server`).
    #[serde(default = "default_backend")]
    pub backend: String,
    /// Path to the redb database file. Honored when `backend = "redb"`.
    #[serde(default = "default_db_path")]
    pub path: String,
    /// Postgres connection URL. Honored (and required) when
    /// `backend = "postgres"`.
    #[serde(default)]
    pub url: Option<String>,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            backend: default_backend(),
            path: default_db_path(),
            url: None,
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
    /// Load from a TOML file if it exists, else defaults. Validates the
    /// storage selection so a misconfigured `backend = "postgres"` (with the
    /// feature off, or with a missing `url`) is surfaced at startup rather
    /// than as a downstream connection error.
    pub fn load(path: &str) -> anyhow::Result<Self> {
        let cfg: Self = match std::fs::read_to_string(path) {
            Ok(s) => toml::from_str(&s)?,
            Err(_) => Self::default(),
        };
        cfg.validate_storage()?;
        Ok(cfg)
    }

    /// Validate the `[storage]` selection. Fail-fast errors:
    /// - `backend = "postgres"` requires the `postgres` cargo feature on
    ///   `schemahub-server`; the binary was built without it.
    /// - `backend = "postgres"` requires `storage.url` to be set.
    /// - Any unknown `backend` string.
    fn validate_storage(&self) -> anyhow::Result<()> {
        match self.storage.backend.as_str() {
            "redb" => Ok(()),
            "postgres" => {
                #[cfg(not(feature = "postgres"))]
                {
                    anyhow::bail!(
                        "storage.backend = \"postgres\" requires building schemahub-server \
                         with `--features postgres`; this binary was built without it"
                    );
                }
                #[cfg(feature = "postgres")]
                {
                    if self.storage.url.as_deref().unwrap_or("").is_empty() {
                        anyhow::bail!(
                            "storage.backend = \"postgres\" requires storage.url \
                             (e.g. postgres://user:pass@host:5432/dbname)"
                        );
                    }
                    Ok(())
                }
            }
            other => anyhow::bail!(
                "unknown storage.backend {other:?}; expected \"redb\" or \"postgres\""
            ),
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
