use serde::{Deserialize, Serialize};
use schemahub_types::CompatibilityDirection;
use schemahub_storage::StorageBackend;

use crate::error::CoreError;

fn compat_direction_to_str(d: CompatibilityDirection) -> &'static str {
    match d {
        CompatibilityDirection::Backward => "backward",
        CompatibilityDirection::Forward  => "forward",
        CompatibilityDirection::Full     => "full",
        CompatibilityDirection::Disabled => "disabled",
    }
}

fn compat_direction_from_str(s: &str) -> CompatibilityDirection {
    match s {
        "backward" => CompatibilityDirection::Backward,
        "forward"  => CompatibilityDirection::Forward,
        "full"     => CompatibilityDirection::Full,
        _          => CompatibilityDirection::Disabled,
    }
}

mod compat_serde {
    use serde::{Deserializer, Serializer, Deserialize};
    use schemahub_types::CompatibilityDirection;
    use super::{compat_direction_to_str, compat_direction_from_str};

    pub fn serialize<S: Serializer>(d: &CompatibilityDirection, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(compat_direction_to_str(*d))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<CompatibilityDirection, D::Error> {
        let s = String::deserialize(d)?;
        Ok(compat_direction_from_str(&s))
    }
}

/// Repository configuration, stored as JSON under `config/<project>/<repo>`.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RepoConfig {
    pub default_branch: String,
    #[serde(with = "compat_serde")]
    pub compatibility_direction: CompatibilityDirection,
    /// Branch names (or glob patterns like "release/*") that are protected.
    pub protected_branches: Vec<String>,
}

impl Default for RepoConfig {
    fn default() -> Self {
        Self {
            default_branch: "main".to_string(),
            compatibility_direction: CompatibilityDirection::Backward,
            protected_branches: vec!["main".to_string()],
        }
    }
}

impl RepoConfig {
    /// Returns true if the given branch name is protected.
    /// Supports exact match and simple "prefix/*" glob patterns.
    pub fn is_protected(&self, branch: &str) -> bool {
        self.protected_branches.iter().any(|pattern| {
            if let Some(prefix) = pattern.strip_suffix("/*") {
                branch.starts_with(&format!("{prefix}/"))
            } else {
                pattern == branch
            }
        })
    }
}

fn config_key(project: &str, repo: &str) -> String {
    format!("config/{project}/{repo}")
}

/// Load the RepoConfig from storage. Returns the default if not found.
pub fn load_repo_config(
    storage: &dyn StorageBackend,
    project: &str,
    repo: &str,
) -> Result<RepoConfig, CoreError> {
    let key = config_key(project, repo);
    match storage.get(&key)? {
        None => Ok(RepoConfig::default()),
        Some(bytes) => {
            serde_json::from_slice(&bytes)
                .map_err(|e| CoreError::ObjectCorrupted(format!("repo config is malformed: {e}")))
        }
    }
}

/// Persist a RepoConfig to storage.
pub fn save_repo_config(
    storage: &dyn StorageBackend,
    project: &str,
    repo: &str,
    config: &RepoConfig,
) -> Result<(), CoreError> {
    let key = config_key(project, repo);
    let bytes = serde_json::to_vec(config)
        .map_err(|e| CoreError::InvalidArgument(format!("failed to serialize repo config: {e}")))?;
    storage.put(&key, &bytes)?;
    Ok(())
}
