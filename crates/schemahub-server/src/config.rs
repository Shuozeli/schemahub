//! Server configuration (crate-structure.md §3.6): db backend + path, listen
//! address, HTTP boundary policy, bootstrap per-repo compatibility config,
//! and `[auth]` + `[projects.<name>]` sections that drive the RBAC layer
//! (design.md §6). Loaded from `schemahub.toml` (optional) and overridable by
//! CLI flags.

use std::collections::HashMap;
use std::collections::HashSet;
use std::str::FromStr;

use jsonwebtoken::{Algorithm, AlgorithmFamily};
use schemahub_core::{
    repository::validate_config, RepoConfig, RepoConfigStore, ReviewPolicy, ServingPolicy,
};
use schemahub_types::{CompatibilityDirection, Identity, IdentityKind, Role, Visibility};
use serde::Deserialize;

pub const DEFAULT_HTTP_MAX_REQUEST_BODY_BYTES: usize = 8 * 1_024 * 1_024;

/// Top-level server config, deserialized from `schemahub.toml`.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub storage: StorageConfig,
    #[serde(default)]
    pub listen: ListenConfig,
    /// Browser-facing HTTP boundary policy. Cross-origin access is disabled
    /// unless an exact origin is listed; request bodies are always bounded.
    #[serde(default)]
    pub http: HttpConfig,
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

/// `[http]` policy for the optional browser BFF listener.
///
/// An empty origin list is the secure same-origin default: SchemaHub emits no
/// CORS response headers. Operators that host the GUI on another origin must
/// list each trusted origin exactly. Credentials/cookies are never enabled.
#[derive(Debug, Clone, Deserialize)]
pub struct HttpConfig {
    #[serde(default)]
    pub allowed_origins: Vec<String>,
    #[serde(default = "default_http_max_request_body_bytes")]
    pub max_request_body_bytes: usize,
}

impl Default for HttpConfig {
    fn default() -> Self {
        Self {
            allowed_origins: Vec::new(),
            max_request_body_bytes: default_http_max_request_body_bytes(),
        }
    }
}

impl HttpConfig {
    pub(crate) fn validate(&self) -> anyhow::Result<()> {
        if !(1_024..=67_108_864).contains(&self.max_request_body_bytes) {
            anyhow::bail!("[http].max_request_body_bytes must be between 1024 and 67108864");
        }

        let mut unique = HashSet::with_capacity(self.allowed_origins.len());
        for configured in &self.allowed_origins {
            let parsed = reqwest::Url::parse(configured).map_err(|error| {
                anyhow::anyhow!(
                    "[http].allowed_origins contains invalid origin {configured:?}: {error}"
                )
            })?;
            if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
                anyhow::bail!(
                    "[http].allowed_origins entry {configured:?} must be an absolute HTTP(S) origin"
                );
            }
            if !parsed.username().is_empty()
                || parsed.password().is_some()
                || parsed.path() != "/"
                || parsed.query().is_some()
                || parsed.fragment().is_some()
            {
                anyhow::bail!(
                    "[http].allowed_origins entry {configured:?} must not contain credentials, a path, query, or fragment"
                );
            }
            let canonical = parsed.origin().ascii_serialization();
            if configured != &canonical {
                anyhow::bail!(
                    "[http].allowed_origins entry {configured:?} is not canonical; use {canonical:?}"
                );
            }
            if !unique.insert(configured) {
                anyhow::bail!("[http].allowed_origins contains duplicate origin {configured:?}");
            }
        }
        Ok(())
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
    #[serde(default)]
    pub review: Option<RepoReviewSection>,
    #[serde(default)]
    pub serving: Option<RepoServingSection>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct RepoReviewSection {
    #[serde(default)]
    pub required_approvals: Option<u32>,
    #[serde(default)]
    pub require_change_record: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct RepoServingSection {
    #[serde(default)]
    pub source: Option<bool>,
    #[serde(default)]
    pub descriptors: Option<bool>,
    #[serde(default)]
    pub generated_code: Option<bool>,
}

/// `[auth]` section. Static tokens are intended for development; `[auth.jwt]`
/// configures production JWT verification against a rotating JWKS. The two
/// credential modes are deliberately mutually exclusive.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct AuthConfig {
    /// Legacy JSON access-store directory. On first database-backed start,
    /// existing `projects.json` and `roles.json` files here are imported
    /// atomically. New project and membership writes use the selected database.
    #[serde(default = "default_auth_data_dir")]
    pub data_dir: String,
    /// Static bearer-token table: token → identity. When absent or empty, the
    /// server installs `NoopAuthn` + `NoopAuthz` (anonymous everything).
    #[serde(default)]
    pub tokens: HashMap<String, TokenIdentity>,
    /// OIDC-compatible JWT resource-server configuration. Every field inside
    /// this block is required so production security choices stay explicit.
    #[serde(default)]
    pub jwt: Option<JwtAuthConfig>,
}

/// Production bearer-JWT verification and JWKS refresh policy.
///
/// `jwks_url` and `jwks_file` are mutually exclusive. HTTPS is mandatory for
/// remote key retrieval; a file source supports air-gapped deployments and
/// deterministic acceptance tests. Token time is checked through an injected
/// clock in `jwt_auth`, rather than through the JWT library's system clock.
#[derive(Debug, Clone, Deserialize)]
pub struct JwtAuthConfig {
    pub issuer: String,
    pub audiences: Vec<String>,
    pub algorithms: Vec<String>,
    pub token_type: String,
    pub identity_id_prefix: String,
    #[serde(default)]
    pub jwks_url: Option<String>,
    #[serde(default)]
    pub jwks_file: Option<String>,
    pub clock_skew_seconds: u64,
    pub refresh_interval_seconds: u64,
    pub max_stale_seconds: u64,
    pub request_timeout_seconds: u64,
    pub max_token_bytes: usize,
    pub max_jwks_bytes: usize,
}

impl JwtAuthConfig {
    pub fn parsed_algorithms(&self) -> anyhow::Result<Vec<Algorithm>> {
        let mut parsed = Vec::with_capacity(self.algorithms.len());
        let mut unique = HashSet::with_capacity(self.algorithms.len());
        for configured in &self.algorithms {
            let algorithm = Algorithm::from_str(configured).map_err(|_| {
                anyhow::anyhow!(
                    "[auth.jwt].algorithms contains unsupported algorithm {configured:?}"
                )
            })?;
            if algorithm.family() == AlgorithmFamily::Hmac {
                anyhow::bail!(
                    "[auth.jwt].algorithms must use asymmetric signatures; {configured:?} is HMAC"
                );
            }
            if !unique.insert(algorithm) {
                anyhow::bail!("[auth.jwt].algorithms contains duplicate algorithm {configured:?}");
            }
            parsed.push(algorithm);
        }
        if parsed.is_empty() {
            anyhow::bail!("[auth.jwt].algorithms must contain at least one algorithm");
        }
        Ok(parsed)
    }

    fn validate(&self) -> anyhow::Result<()> {
        let issuer = reqwest::Url::parse(&self.issuer)
            .map_err(|error| anyhow::anyhow!("[auth.jwt].issuer is not a URL: {error}"))?;
        if issuer.scheme() != "https" || issuer.host_str().is_none() {
            anyhow::bail!("[auth.jwt].issuer must be an absolute HTTPS URL");
        }
        if issuer.query().is_some() || issuer.fragment().is_some() {
            anyhow::bail!("[auth.jwt].issuer must not contain a query or fragment");
        }
        if self.audiences.is_empty()
            || self.audiences.iter().any(|audience| {
                audience.trim().is_empty() || audience.chars().any(char::is_control)
            })
        {
            anyhow::bail!("[auth.jwt].audiences must contain only non-empty values");
        }
        if self.audiences.iter().collect::<HashSet<_>>().len() != self.audiences.len() {
            anyhow::bail!("[auth.jwt].audiences must not contain duplicates");
        }
        self.parsed_algorithms()?;
        if self.token_type.trim().is_empty()
            || self.token_type.len() > 64
            || !self.token_type.is_ascii()
            || self.token_type.chars().any(char::is_whitespace)
            || self.token_type.chars().any(char::is_control)
        {
            anyhow::bail!(
                "[auth.jwt].token_type must be 1..=64 non-whitespace printable ASCII bytes"
            );
        }
        if self.identity_id_prefix.trim().is_empty()
            || self.identity_id_prefix.len() > 256
            || self.identity_id_prefix.chars().any(char::is_control)
        {
            anyhow::bail!("[auth.jwt].identity_id_prefix must be 1..=256 bytes");
        }
        match (&self.jwks_url, &self.jwks_file) {
            (Some(url), None) => {
                let url = reqwest::Url::parse(url).map_err(|error| {
                    anyhow::anyhow!("[auth.jwt].jwks_url is not a URL: {error}")
                })?;
                if url.scheme() != "https" || url.host_str().is_none() {
                    anyhow::bail!("[auth.jwt].jwks_url must be an absolute HTTPS URL");
                }
                if !url.username().is_empty()
                    || url.password().is_some()
                    || url.query().is_some()
                    || url.fragment().is_some()
                {
                    anyhow::bail!(
                        "[auth.jwt].jwks_url must not contain credentials, a query, or a fragment"
                    );
                }
            }
            (None, Some(path)) if !path.trim().is_empty() => {}
            (None, Some(_)) => anyhow::bail!("[auth.jwt].jwks_file must not be empty"),
            (Some(_), Some(_)) => anyhow::bail!(
                "[auth.jwt] must configure exactly one of jwks_url or jwks_file, not both"
            ),
            (None, None) => {
                anyhow::bail!("[auth.jwt] must configure exactly one of jwks_url or jwks_file")
            }
        }
        if self.clock_skew_seconds > 300 {
            anyhow::bail!("[auth.jwt].clock_skew_seconds must be at most 300");
        }
        if self.refresh_interval_seconds < 30 {
            anyhow::bail!("[auth.jwt].refresh_interval_seconds must be at least 30");
        }
        let minimum_stale = self
            .refresh_interval_seconds
            .checked_mul(2)
            .ok_or_else(|| anyhow::anyhow!("[auth.jwt].refresh_interval_seconds is too large"))?;
        if self.max_stale_seconds < minimum_stale {
            anyhow::bail!(
                "[auth.jwt].max_stale_seconds must be at least twice refresh_interval_seconds"
            );
        }
        if !(1..=60).contains(&self.request_timeout_seconds) {
            anyhow::bail!("[auth.jwt].request_timeout_seconds must be between 1 and 60");
        }
        if !(1_024..=65_536).contains(&self.max_token_bytes) {
            anyhow::bail!("[auth.jwt].max_token_bytes must be between 1024 and 65536");
        }
        if !(1_024..=4_194_304).contains(&self.max_jwks_bytes) {
            anyhow::bail!("[auth.jwt].max_jwks_bytes must be between 1024 and 4194304");
        }
        Ok(())
    }
}

/// Identity associated with a bearer token in `[auth].tokens`.
#[derive(Debug, Clone, Deserialize)]
pub struct TokenIdentity {
    pub id: String,
    #[serde(default)]
    pub display: Option<String>,
    /// Audit principal kind. Existing configurations without this field remain
    /// human identities; accepted values are `human`, `agent`, and `service`.
    #[serde(default = "default_identity_kind")]
    pub kind: IdentityKind,
    /// Optional delegating human/service identity for agent credentials.
    #[serde(default)]
    pub delegated_by: Option<String>,
}

impl TokenIdentity {
    pub fn to_identity(&self) -> Identity {
        match self.kind {
            IdentityKind::Human => match &self.display {
                Some(d) => Identity::user_with_display(&self.id, d),
                None => Identity::user(&self.id),
            },
            IdentityKind::Agent => {
                Identity::agent(&self.id, self.display.clone(), self.delegated_by.clone())
            }
            IdentityKind::Service => Identity::service(&self.id, self.display.clone()),
            IdentityKind::Anonymous => {
                unreachable!("validated token identity cannot be anonymous")
            }
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
fn default_http_max_request_body_bytes() -> usize {
    DEFAULT_HTTP_MAX_REQUEST_BODY_BYTES
}
fn default_auth_data_dir() -> String {
    "schemahub-data".to_string()
}
fn default_identity_kind() -> IdentityKind {
    IdentityKind::Human
}

impl Config {
    /// Load a required TOML file. Missing, unreadable, and malformed files all
    /// fail closed so an explicitly configured authentication policy cannot
    /// silently become anonymous access.
    pub fn load(path: &str) -> anyhow::Result<Self> {
        Self::load_inner(path, false)
    }

    /// Load the conventional optional config path, using defaults only when
    /// the file does not exist. Other filesystem failures still fail closed.
    pub fn load_optional(path: &str) -> anyhow::Result<Self> {
        Self::load_inner(path, true)
    }

    fn load_inner(path: &str, allow_missing: bool) -> anyhow::Result<Self> {
        let cfg: Self = match std::fs::read_to_string(path) {
            Ok(s) => toml::from_str(&s)?,
            Err(error) if allow_missing && error.kind() == std::io::ErrorKind::NotFound => {
                Self::default()
            }
            Err(error) => anyhow::bail!("reading server config {path:?}: {error}"),
        };
        cfg.validate_storage()?;
        cfg.http.validate()?;
        cfg.validate_auth()?;
        cfg.validate_repositories()?;
        Ok(cfg)
    }

    /// Validate the `[auth]` + `[projects.*]` sections. Fails fast on:
    /// - an unknown project visibility string,
    /// - an unknown role in `[projects.*].members`,
    /// - a missing `id` in a `[auth].tokens` entry (deserialization would
    ///   catch this, but we re-check so the error mentions which token).
    fn validate_auth(&self) -> anyhow::Result<()> {
        if self.auth.jwt.is_some() && !self.auth.tokens.is_empty() {
            anyhow::bail!("[auth].tokens and [auth.jwt] are mutually exclusive credential modes");
        }
        if let Some(jwt) = &self.auth.jwt {
            jwt.validate()?;
        }
        for (token, identity) in &self.auth.tokens {
            if identity.id.is_empty() {
                anyhow::bail!("[auth].tokens.{token:?}: identity 'id' must not be empty");
            }
            if identity.kind == IdentityKind::Anonymous {
                anyhow::bail!(
                    "[auth].tokens.{token:?}: identity kind must be human, agent, or service"
                );
            }
            if identity.delegated_by.is_some() && identity.kind != IdentityKind::Agent {
                anyhow::bail!(
                    "[auth].tokens.{token:?}: delegated_by is valid only for agent identities"
                );
            }
        }
        for (name, section) in &self.projects {
            if section.owners.is_empty() {
                anyhow::bail!("[projects.{name}] must configure at least one owner identity");
            }
            if section.owners.iter().any(String::is_empty) {
                anyhow::bail!("[projects.{name}].owners must not contain an empty identity");
            }
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
        !self.auth.tokens.is_empty() || self.auth.jwt.is_some() || !self.projects.is_empty()
    }

    /// Stable identifier surfaced by the server-config and readiness APIs.
    pub fn auth_mode(&self) -> &'static str {
        if self.auth.jwt.is_some() {
            "jwt-rbac"
        } else if self.auth_enabled() {
            "static-bearer-rbac"
        } else {
            "noop"
        }
    }

    fn validate_repositories(&self) -> anyhow::Result<()> {
        for (key, section) in &self.repos {
            let Some((project, repo)) = key.split_once('/') else {
                anyhow::bail!("[repos.{key:?}] must be keyed by exactly one project/repo pair");
            };
            if project.is_empty() || repo.is_empty() || repo.contains('/') {
                anyhow::bail!(
                    "[repos.{key:?}] must be keyed by exactly one non-empty project/repo pair"
                );
            }
            if let Some(direction) = &section.compatibility {
                parse_direction_strict(direction)
                    .map_err(|error| anyhow::anyhow!("[repos.{key:?}]: {error}"))?;
            }
            let config = repo_config_from_section(section);
            validate_config(&config)
                .map_err(|error| anyhow::anyhow!("[repos.{key:?}]: {error}"))?;
        }
        Ok(())
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
            store.set(project, repo, repo_config_from_section(section));
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

fn parse_direction_strict(s: &str) -> anyhow::Result<CompatibilityDirection> {
    match s.to_lowercase().as_str() {
        "backward" => Ok(CompatibilityDirection::Backward),
        "forward" => Ok(CompatibilityDirection::Forward),
        "full" => Ok(CompatibilityDirection::Full),
        "disabled" | "none" => Ok(CompatibilityDirection::Disabled),
        _ => anyhow::bail!("compatibility must be backward, forward, full, disabled, or none"),
    }
}

fn repo_config_from_section(section: &RepoSection) -> RepoConfig {
    let mut config = RepoConfig::default();
    if let Some(bookmark) = &section.default_bookmark {
        config.default_bookmark = bookmark.clone();
    }
    if let Some(compatibility) = &section.compatibility {
        config.compatibility_direction = parse_direction(compatibility);
    }
    if let Some(protected) = &section.protected_bookmarks {
        config.protected_bookmarks = protected.clone();
    }
    if let Some(review) = &section.review {
        config.review_policy = ReviewPolicy {
            required_approvals: review.required_approvals.unwrap_or_default(),
            require_change_record: review.require_change_record.unwrap_or_default(),
        };
    }
    if let Some(serving) = &section.serving {
        let defaults = ServingPolicy::default();
        config.serving_policy = ServingPolicy {
            source: serving.source.unwrap_or(defaults.source),
            descriptors: serving.descriptors.unwrap_or(defaults.descriptors),
            generated_code: serving.generated_code.unwrap_or(defaults.generated_code),
        };
    }
    config
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_optional_config_uses_safe_defaults() {
        // Arrange
        let temp = tempfile::tempdir().expect("config tempdir");
        let missing = temp.path().join("missing-schemahub.toml");

        // Act
        let config = Config::load_optional(missing.to_str().expect("UTF-8 test path"))
            .expect("missing optional config should use defaults");

        // Assert
        assert_eq!(config.auth_mode(), "noop");
        assert_eq!(config.storage.backend, "redb");
        assert!(config.http.allowed_origins.is_empty());
        assert_eq!(config.http.max_request_body_bytes, 8 * 1_024 * 1_024);
    }

    #[test]
    fn http_policy_accepts_canonical_trusted_origins_and_bounded_bodies() {
        // Arrange
        let config: Config = toml::from_str(
            r#"
[http]
allowed_origins = ["https://schemahub.example.test", "http://gui.example.test:5173"]
max_request_body_bytes = 16777216
"#,
        )
        .expect("parse HTTP policy");

        // Act
        let result = config.http.validate();

        // Assert
        assert!(result.is_ok());
    }

    #[test]
    fn http_policy_rejects_an_origin_with_a_path() {
        // Arrange
        let config = HttpConfig {
            allowed_origins: vec!["https://schemahub.example.test/app".to_string()],
            ..HttpConfig::default()
        };

        // Act
        let result = config.validate();

        // Assert
        assert!(result
            .expect_err("origin paths must fail")
            .to_string()
            .contains("must not contain credentials, a path, query, or fragment"));
    }

    #[test]
    fn http_policy_rejects_duplicate_origins() {
        // Arrange
        let config = HttpConfig {
            allowed_origins: vec![
                "https://schemahub.example.test".to_string(),
                "https://schemahub.example.test".to_string(),
            ],
            ..HttpConfig::default()
        };

        // Act
        let result = config.validate();

        // Assert
        assert!(result
            .expect_err("duplicate origins must fail")
            .to_string()
            .contains("duplicate origin"));
    }

    #[test]
    fn http_policy_rejects_a_request_body_limit_below_one_kibibyte() {
        // Arrange
        let config = HttpConfig {
            max_request_body_bytes: 1_023,
            ..HttpConfig::default()
        };

        // Act
        let result = config.validate();

        // Assert
        assert!(result
            .expect_err("undersized body limit must fail")
            .to_string()
            .contains("between 1024 and 67108864"));
    }

    #[test]
    fn http_policy_rejects_a_request_body_limit_above_sixty_four_mibibytes() {
        // Arrange
        let config = HttpConfig {
            max_request_body_bytes: 67_108_865,
            ..HttpConfig::default()
        };

        // Act
        let result = config.validate();

        // Assert
        assert!(result
            .expect_err("oversized body limit must fail")
            .to_string()
            .contains("between 1024 and 67108864"));
    }

    #[test]
    fn missing_explicit_config_fails_closed() {
        // Arrange
        let temp = tempfile::tempdir().expect("config tempdir");
        let missing = temp.path().join("missing-schemahub.toml");

        // Act
        let error = Config::load(missing.to_str().expect("UTF-8 test path"))
            .expect_err("missing explicit config must fail");

        // Assert
        assert!(error.to_string().contains("reading server config"));
    }

    #[test]
    fn unreadable_config_source_fails_closed() {
        // Arrange
        let temp = tempfile::tempdir().expect("config tempdir");
        let directory_path = temp.path().to_str().expect("UTF-8 test path");

        // Act
        let error = Config::load(directory_path).expect_err("directory is not a config file");

        // Assert
        assert!(error.to_string().contains("reading server config"));
    }

    #[test]
    fn agent_token_identity_preserves_kind_and_delegation() {
        // Arrange
        let configured = TokenIdentity {
            id: "schema-agent".to_string(),
            display: Some("Schema Agent".to_string()),
            kind: IdentityKind::Agent,
            delegated_by: Some("alice".to_string()),
        };

        // Act
        let identity = configured.to_identity();

        // Assert
        assert_eq!(identity.id(), Some("schema-agent"));
        assert_eq!(identity.kind(), IdentityKind::Agent);
        assert_eq!(identity.display(), Some("Schema Agent"));
        assert_eq!(identity.delegated_by(), Some("alice"));
    }

    #[test]
    fn human_is_the_backward_compatible_token_identity_kind() {
        // Arrange
        let configured: TokenIdentity = toml::from_str(
            r#"
id = "alice"
display = "Alice"
"#,
        )
        .expect("parse token identity");

        // Act
        let identity = configured.to_identity();

        // Assert
        assert_eq!(identity.kind(), IdentityKind::Human);
        assert_eq!(identity.id(), Some("alice"));
    }

    #[test]
    fn repository_config_rejects_unknown_compatibility_at_startup() {
        // Arrange
        let config: Config = toml::from_str(
            r#"
[repos."acme/commerce"]
compatibility = "mostly"
"#,
        )
        .expect("parse config shape");

        // Act
        let result = config.validate_repositories();

        // Assert
        assert!(result
            .expect_err("unknown compatibility must fail")
            .to_string()
            .contains("compatibility must be"));
    }

    #[test]
    fn repository_config_rejects_malformed_registry_key_at_startup() {
        // Arrange
        let config: Config = toml::from_str(
            r#"
[repos."acme/commerce/extra"]
default_bookmark = "main"
"#,
        )
        .expect("parse config shape");

        // Act
        let result = config.validate_repositories();

        // Assert
        assert!(result
            .expect_err("malformed key must fail")
            .to_string()
            .contains("exactly one non-empty project/repo pair"));
    }

    #[test]
    fn repository_review_and_serving_policy_parse_with_safe_defaults() {
        // Arrange
        let config: Config = toml::from_str(
            r#"
[repos."acme/commerce"]
compatibility = "backward"

[repos."acme/commerce".review]
required_approvals = 2
require_change_record = true

[repos."acme/commerce".serving]
source = false
"#,
        )
        .expect("parse repository policy");

        // Act
        config
            .validate_repositories()
            .expect("valid repository config");
        let stored = config.repo_config_store().get("acme", "commerce");

        // Assert
        assert_eq!(stored.review_policy.required_approvals, 2);
        assert!(stored.review_policy.require_change_record);
        assert!(!stored.serving_policy.source);
        assert!(stored.serving_policy.descriptors);
        assert!(stored.serving_policy.generated_code);
    }

    #[test]
    fn production_jwt_config_requires_and_accepts_explicit_security_policy() {
        // Arrange
        let config: Config = toml::from_str(
            r#"
[auth.jwt]
issuer = "https://identity.example.test"
audiences = ["schemahub"]
algorithms = ["RS256", "EdDSA"]
token_type = "at+jwt"
identity_id_prefix = "oidc:"
jwks_url = "https://identity.example.test/.well-known/jwks.json"
clock_skew_seconds = 30
refresh_interval_seconds = 300
max_stale_seconds = 1800
request_timeout_seconds = 5
max_token_bytes = 8192
max_jwks_bytes = 1048576
"#,
        )
        .expect("parse JWT config");

        // Act
        let result = config.validate_auth();

        // Assert
        assert!(result.is_ok());
        assert_eq!(config.auth_mode(), "jwt-rbac");
        assert_eq!(
            config
                .auth
                .jwt
                .expect("JWT config")
                .parsed_algorithms()
                .expect("algorithms"),
            vec![Algorithm::RS256, Algorithm::EdDSA]
        );
    }

    #[test]
    fn jwt_and_static_tokens_are_rejected_as_ambiguous_credentials() {
        // Arrange
        let config: Config = toml::from_str(
            r#"
[auth.tokens.dev]
id = "alice"

[auth.jwt]
issuer = "https://identity.example.test"
audiences = ["schemahub"]
algorithms = ["RS256"]
token_type = "at+jwt"
identity_id_prefix = "oidc:"
jwks_file = "jwks.json"
clock_skew_seconds = 30
refresh_interval_seconds = 300
max_stale_seconds = 1800
request_timeout_seconds = 5
max_token_bytes = 8192
max_jwks_bytes = 1048576
"#,
        )
        .expect("parse mixed config");

        // Act
        let result = config.validate_auth();

        // Assert
        assert!(result
            .expect_err("mixed credential modes must fail")
            .to_string()
            .contains("mutually exclusive"));
    }

    #[test]
    fn jwt_config_rejects_hmac_algorithms() {
        // Arrange
        let config: Config = toml::from_str(
            r#"
[auth.jwt]
issuer = "https://identity.example.test"
audiences = ["schemahub"]
algorithms = ["HS256"]
token_type = "at+jwt"
identity_id_prefix = "oidc:"
jwks_url = "http://identity.example.test/jwks.json"
clock_skew_seconds = 30
refresh_interval_seconds = 300
max_stale_seconds = 1800
request_timeout_seconds = 5
max_token_bytes = 8192
max_jwks_bytes = 1048576
"#,
        )
        .expect("parse insecure config");

        // Act
        let result = config.validate_auth();

        // Assert
        assert!(result
            .expect_err("HMAC must fail")
            .to_string()
            .contains("asymmetric signatures"));
    }

    #[test]
    fn jwt_config_rejects_insecure_remote_keys() {
        // Arrange
        let config: Config = toml::from_str(
            r#"
[auth.jwt]
issuer = "https://identity.example.test"
audiences = ["schemahub"]
algorithms = ["RS256"]
token_type = "at+jwt"
identity_id_prefix = "oidc:"
jwks_url = "http://identity.example.test/jwks.json"
clock_skew_seconds = 30
refresh_interval_seconds = 300
max_stale_seconds = 1800
request_timeout_seconds = 5
max_token_bytes = 8192
max_jwks_bytes = 1048576
"#,
        )
        .expect("parse insecure config");

        // Act
        let result = config.validate_auth();

        // Assert
        assert!(result
            .expect_err("HTTP JWKS must fail")
            .to_string()
            .contains("absolute HTTPS URL"));
    }
}
