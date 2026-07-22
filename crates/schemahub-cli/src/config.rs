use anyhow::Context as _;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize, Serialize, Default)]
pub struct ProfileConfig {
    pub server: Option<String>,
    pub token: Option<String>,
}

#[derive(Debug, Deserialize, Serialize, Default)]
struct RawConfig {
    #[serde(flatten)]
    profiles: HashMap<String, ProfileConfig>,
}

/// Resolved configuration for a single operation.
pub struct Config {
    pub server: String,
    pub token: String,
}

impl Config {
    pub fn load(
        profile: &str,
        server_override: Option<&str>,
        token_override: Option<&str>,
    ) -> anyhow::Result<Self> {
        let raw = load_raw_config()?;
        Self::resolve(raw, profile, server_override, token_override)
    }

    fn resolve(
        raw: RawConfig,
        profile: &str,
        server_override: Option<&str>,
        token_override: Option<&str>,
    ) -> anyhow::Result<Self> {
        let profile_data = raw
            .profiles
            .get(profile)
            .map(|p| (p.server.as_deref(), p.token.as_deref()))
            .unwrap_or((None, None));

        let server = server_override
            .or(profile_data.0)
            .filter(|value| !value.trim().is_empty())
            .context(
                "server address is required; set --server, SCHEMAHUB_SERVER, or profile.server",
            )?
            .to_string();

        let token = token_override.or(profile_data.1).unwrap_or("").to_string();

        Ok(Config { server, token })
    }
}

fn load_raw_config() -> anyhow::Result<RawConfig> {
    let path = match dirs_path() {
        Some(p) => p,
        None => return Ok(RawConfig::default()),
    };
    load_raw_config_from(&path)
}

fn load_raw_config_from(path: &Path) -> anyhow::Result<RawConfig> {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(RawConfig::default());
        }
        Err(error) => {
            return Err(error).with_context(|| format!("reading CLI config {}", path.display()));
        }
    };
    toml::from_str(&content).with_context(|| format!("parsing CLI config {}", path.display()))
}

fn dirs_path() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(PathBuf::from(home).join(".schemahub").join("config"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_config_is_allowed_when_the_server_is_explicit() {
        // Arrange
        let directory = tempfile::tempdir().expect("config tempdir");
        let path = directory.path().join("missing-config");

        // Act
        let raw = load_raw_config_from(&path).expect("missing config should be optional");
        let config = Config::resolve(raw, "default", Some("https://schemahub.example.com"), None)
            .expect("explicit server should resolve");

        // Assert
        assert_eq!(config.server, "https://schemahub.example.com");
        assert!(config.token.is_empty());
    }

    #[test]
    fn malformed_config_is_rejected_even_with_command_line_overrides() {
        // Arrange
        let directory = tempfile::tempdir().expect("config tempdir");
        let path = directory.path().join("config");
        std::fs::write(&path, "[default\nserver = false").expect("write malformed config");

        // Act
        let error = load_raw_config_from(&path).expect_err("malformed TOML must fail closed");

        // Assert
        assert!(error.to_string().contains("parsing CLI config"));
    }

    #[test]
    fn missing_server_has_no_loopback_fallback() {
        // Arrange
        let raw = RawConfig::default();

        // Act
        let error = Config::resolve(raw, "default", None, None)
            .err()
            .expect("a server coordinate must be explicit");

        // Assert
        assert!(error.to_string().contains("server address is required"));
    }

    #[test]
    fn unreadable_config_path_is_not_treated_as_missing() {
        // Arrange
        let directory = tempfile::tempdir().expect("config tempdir");

        // Act
        let error = load_raw_config_from(directory.path())
            .expect_err("reading a directory as config must fail");

        // Assert
        assert!(error.to_string().contains("reading CLI config"));
    }
}
