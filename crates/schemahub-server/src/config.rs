//! Server configuration (crate-structure.md §3.6): db backend + path, listen
//! address, bootstrap per-repo compatibility config, and (new in P0)
//! `[auth]` + `[projects.<name>]` sections that drive the RBAC layer
//! (design.md §6). Loaded from `schemahub.toml` (optional) and overridable
//! by CLI flags.

use std::collections::HashMap;

use schemahub_core::{RepoConfig, RepoConfigStore};
use schemahub_types::{CompatibilityDirection, Identity, Role, Visibility};
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
    /// AuthN/AuthZ configuration. When absent or empty, the server installs
    /// the Noop providers and the RBAC layer is effectively off (today's
    /// behavior — every request is anonymous, every action allowed).
    #[serde(default)]
    pub auth: AuthConfig,
    /// Project bootstrap, keyed by project name. Seeds the project + role
    /// registries on startup (idempotent: if the registries already hold an
    /// entry, the bootstrap does not overwrite it).
    #[serde(default)]
    pub projects: HashMap<String, ProjectSection>,
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

/// `[auth]` section — wires the real `BearerTokenAuthn` + `RoleBasedAuthz`
/// when populated. Empty / absent ⇒ Noop providers (today's behavior).
#[derive(Debug, Clone, Deserialize, Default)]
pub struct AuthConfig {
    /// Directory for the file-backed role + project stores (created on first
    /// write). Defaults to "schemahub-data". Honored only when at least one
    /// auth-related field is present.
    #[serde(default = "default_auth_data_dir")]
    pub data_dir: String,
    /// Static bearer-token table: token → identity. When absent or empty, the
    /// server installs `NoopAuthn` + `NoopAuthz` (anonymous everything).
    #[serde(default)]
    pub tokens: HashMap<String, TokenIdentity>,
}

/// Identity associated with a bearer token in `[auth].tokens`.
#[derive(Debug, Clone, Deserialize)]
pub struct TokenIdentity {
    pub id: String,
    #[serde(default)]
    pub display: Option<String>,
}

impl TokenIdentity {
    pub fn to_identity(&self) -> Identity {
        match &self.display {
            Some(d) => Identity::user_with_display(&self.id, d),
            None => Identity::user(&self.id),
        }
    }
}

/// `[projects.<name>]` bootstrap. Seeds the project + role registries at
/// startup so an admin doesn't have to call `CreateProject` / `AddMember` by
/// hand for every project the server should know about on day one.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ProjectSection {
    /// "public" | "private". Defaults to "private".
    #[serde(default)]
    pub visibility: Option<String>,
    /// Identity ids to install as Owners on first boot. Each id is set to
    /// `Role::Owner` in the role store.
    #[serde(default)]
    pub owners: Vec<String>,
    /// Optional starter members keyed `identity_id = "<role>"`.
    #[serde(default)]
    pub members: HashMap<String, String>,
}

impl ProjectSection {
    /// Resolve the configured visibility string. Unknown / missing values
    /// fall back to Private (fail-closed).
    pub fn parsed_visibility(&self) -> Visibility {
        match self
            .visibility
            .as_deref()
            .unwrap_or("private")
            .to_ascii_lowercase()
            .as_str()
        {
            "public" => Visibility::Public,
            _ => Visibility::Private,
        }
    }

    /// Parse the `members = { id = "Reader" }` map into typed `(id, role)`
    /// pairs. Returns an error listing every invalid role string so a typo
    /// fails fast at startup rather than silently dropping a member.
    pub fn parsed_members(&self) -> anyhow::Result<Vec<(String, Role)>> {
        let mut out = Vec::with_capacity(self.members.len());
        let mut bad = Vec::new();
        for (id, role_str) in &self.members {
            match Role::parse(role_str) {
                Some(r) => out.push((id.clone(), r)),
                None => bad.push(format!("{id} = {role_str:?}")),
            }
        }
        if !bad.is_empty() {
            anyhow::bail!(
                "unknown role in [projects.*].members: {}; expected one of \
                 Reader / Writer / Maintainer / Owner",
                bad.join(", ")
            );
        }
        Ok(out)
    }
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
fn default_auth_data_dir() -> String {
    "schemahub-data".to_string()
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
        cfg.validate_auth()?;
        Ok(cfg)
    }

    /// Validate the `[auth]` + `[projects.*]` sections. Fails fast on:
    /// - an unknown project visibility string,
    /// - an unknown role in `[projects.*].members`,
    /// - a missing `id` in a `[auth].tokens` entry (deserialization would
    ///   catch this, but we re-check so the error mentions which token).
    fn validate_auth(&self) -> anyhow::Result<()> {
        for (token, identity) in &self.auth.tokens {
            if identity.id.is_empty() {
                anyhow::bail!("[auth].tokens.{token:?}: identity 'id' must not be empty");
            }
        }
        for (name, section) in &self.projects {
            if let Some(v) = &section.visibility {
                match v.to_ascii_lowercase().as_str() {
                    "public" | "private" => {}
                    other => anyhow::bail!(
                        "[projects.{name}].visibility = {other:?}; expected \"public\" or \"private\""
                    ),
                }
            }
            section
                .parsed_members()
                .map_err(|e| anyhow::anyhow!("[projects.{name}]: {e}"))?;
        }
        Ok(())
    }

    /// True when the `[auth]` section has any tokens configured. The server
    /// uses this to decide whether to install the real Bearer/RBAC providers
    /// or fall back to the Noop default.
    pub fn auth_enabled(&self) -> bool {
        !self.auth.tokens.is_empty() || !self.projects.is_empty()
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
