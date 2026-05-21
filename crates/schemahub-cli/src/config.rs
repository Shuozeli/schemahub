use serde::{Deserialize, Serialize};
use std::collections::HashMap;

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
    #[allow(dead_code)]
    pub token: String,
}

impl Config {
    pub fn load(
        profile: &str,
        server_override: Option<&str>,
        token_override: Option<&str>,
    ) -> anyhow::Result<Self> {
        let raw = load_raw_config();

        let profile_data = raw.profiles.get(profile).map(|p| (p.server.as_deref(), p.token.as_deref())).unwrap_or((None, None));

        let server = server_override
            .or(profile_data.0)
            .unwrap_or("http://[::1]:50051")
            .to_string();

        let token = token_override
            .or(profile_data.1)
            .unwrap_or("")
            .to_string();

        Ok(Config { server, token })
    }
}

fn load_raw_config() -> RawConfig {
    let path = match dirs_path() {
        Some(p) => p,
        None => return RawConfig::default(),
    };
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return RawConfig::default(),
    };
    toml::from_str(&content).unwrap_or_default()
}

fn dirs_path() -> Option<std::path::PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(std::path::PathBuf::from(home).join(".schemahub").join("config"))
}

