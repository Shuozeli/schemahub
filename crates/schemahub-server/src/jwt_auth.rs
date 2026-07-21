//! OIDC-compatible JWT authentication with bounded, rotating JWKS state.
//!
//! Request authentication is synchronous because [`AuthnProvider`] is a core
//! boundary. Remote/file key loading happens at startup and in a supervised
//! Tokio refresh task, while requests read an already-validated key cache.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Context;
use jsonwebtoken::jwk::{JwkSet, KeyOperations, PublicKeyUse};
use jsonwebtoken::{decode, decode_header, Algorithm, DecodingKey, Validation};
use reqwest::header::ACCEPT;
use schemahub_types::{AuthnError, AuthnProvider, Identity, IdentityKind};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

use crate::config::JwtAuthConfig;
use crate::http::Readiness;

const AUTH_FRESHNESS_POLL_INTERVAL: Duration = Duration::from_secs(5);

/// Clock boundary used for token validity and key-cache freshness checks.
pub trait JwtClock: Send + Sync + 'static {
    fn now_unix_seconds(&self) -> anyhow::Result<u64>;
}

#[derive(Debug)]
pub struct SystemJwtClock;

impl JwtClock for SystemJwtClock {
    fn now_unix_seconds(&self) -> anyhow::Result<u64> {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("system clock is before the Unix epoch")
            .map(|duration| duration.as_secs())
    }
}

#[derive(Debug, Clone)]
struct JwtValidationPolicy {
    issuer: String,
    audiences: Vec<String>,
    allowed_algorithms: Vec<Algorithm>,
    allowed_algorithm_set: HashSet<Algorithm>,
    token_type: String,
    identity_id_prefix: String,
    clock_skew_seconds: u64,
    max_stale_seconds: u64,
    max_token_bytes: usize,
}

impl JwtValidationPolicy {
    fn from_config(config: &JwtAuthConfig) -> anyhow::Result<Self> {
        let allowed_algorithms = config.parsed_algorithms()?;
        let allowed_algorithm_set = allowed_algorithms.iter().copied().collect();
        Ok(Self {
            issuer: config.issuer.clone(),
            audiences: config.audiences.clone(),
            allowed_algorithms,
            allowed_algorithm_set,
            token_type: config.token_type.clone(),
            identity_id_prefix: config.identity_id_prefix.clone(),
            clock_skew_seconds: config.clock_skew_seconds,
            max_stale_seconds: config.max_stale_seconds,
            max_token_bytes: config.max_token_bytes,
        })
    }
}

#[derive(Debug)]
struct JwtKeyState {
    keys: HashMap<(String, Algorithm), DecodingKey>,
    refreshed_at_unix_seconds: u64,
}

/// Synchronous verifier installed at the core [`AuthnProvider`] boundary.
pub struct JwtAuthn {
    policy: JwtValidationPolicy,
    state: RwLock<JwtKeyState>,
    clock: Arc<dyn JwtClock>,
}

impl JwtAuthn {
    fn new(
        config: &JwtAuthConfig,
        jwks: &JwkSet,
        clock: Arc<dyn JwtClock>,
    ) -> anyhow::Result<Self> {
        let policy = JwtValidationPolicy::from_config(config)?;
        let keys = validated_decoding_keys(jwks, &policy.allowed_algorithm_set)?;
        let refreshed_at_unix_seconds = clock.now_unix_seconds()?;
        Ok(Self {
            policy,
            state: RwLock::new(JwtKeyState {
                keys,
                refreshed_at_unix_seconds,
            }),
            clock,
        })
    }

    fn replace_jwks(&self, jwks: &JwkSet) -> anyhow::Result<usize> {
        // Build and validate the complete replacement before taking the write
        // lock. A malformed refresh never destroys the last known-good cache.
        let keys = validated_decoding_keys(jwks, &self.policy.allowed_algorithm_set)?;
        let count = keys.len();
        let refreshed_at_unix_seconds = self.clock.now_unix_seconds()?;
        let mut state = self
            .state
            .write()
            .map_err(|_| anyhow::anyhow!("JWT key cache lock is poisoned"))?;
        *state = JwtKeyState {
            keys,
            refreshed_at_unix_seconds,
        };
        Ok(count)
    }

    fn is_fresh(&self) -> anyhow::Result<bool> {
        let now = self.clock.now_unix_seconds()?;
        let state = self
            .state
            .read()
            .map_err(|_| anyhow::anyhow!("JWT key cache lock is poisoned"))?;
        Ok(now.saturating_sub(state.refreshed_at_unix_seconds) <= self.policy.max_stale_seconds)
    }

    fn key_count(&self) -> anyhow::Result<usize> {
        self.state
            .read()
            .map(|state| state.keys.len())
            .map_err(|_| anyhow::anyhow!("JWT key cache lock is poisoned"))
    }

    fn identify_token(&self, token: &str) -> Result<Identity, AuthnError> {
        if token.len() > self.policy.max_token_bytes {
            return Err(AuthnError::InvalidToken);
        }

        let header = decode_header(token).map_err(|_| AuthnError::InvalidToken)?;
        if header.typ.as_deref() != Some(self.policy.token_type.as_str())
            || !self.policy.allowed_algorithm_set.contains(&header.alg)
            || header
                .crit
                .as_ref()
                .is_some_and(|critical| !critical.is_empty())
            || header.jku.is_some()
            || header.jwk.is_some()
            || header.x5u.is_some()
            || header.x5c.is_some()
            || header.cty.is_some()
            || header.enc.is_some()
            || header.zip.is_some()
        {
            return Err(AuthnError::InvalidToken);
        }
        let kid = header
            .kid
            .as_deref()
            .filter(|kid| !kid.is_empty())
            .ok_or(AuthnError::InvalidToken)?;

        let now = self
            .clock
            .now_unix_seconds()
            .map_err(|_| AuthnError::Other("authentication clock unavailable".to_string()))?;
        let key = {
            let state = self.state.read().map_err(|_| {
                AuthnError::Other("authentication key cache unavailable".to_string())
            })?;
            if now.saturating_sub(state.refreshed_at_unix_seconds) > self.policy.max_stale_seconds {
                return Err(AuthnError::Other(
                    "authentication key set is stale".to_string(),
                ));
            }
            state
                .keys
                .get(&(kid.to_string(), header.alg))
                .cloned()
                .ok_or(AuthnError::InvalidToken)?
        };

        let mut validation = Validation::new(header.alg);
        validation.algorithms = self.policy.allowed_algorithms.clone();
        validation.set_audience(&self.policy.audiences);
        validation.set_issuer(&[self.policy.issuer.as_str()]);
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        // Time is validated immediately below with the injected clock.
        validation.validate_exp = false;
        validation.validate_nbf = false;
        validation.leeway = 0;

        let claims = decode::<JwtClaims>(token, &key, &validation)
            .map_err(|_| AuthnError::InvalidToken)?
            .claims;
        self.validate_time(&claims, now)?;
        self.identity_from_claims(claims)
    }

    fn validate_time(&self, claims: &JwtClaims, now: u64) -> Result<(), AuthnError> {
        if now.saturating_sub(self.policy.clock_skew_seconds) >= claims.exp {
            return Err(AuthnError::InvalidToken);
        }
        if claims.nbf.is_some_and(|not_before| {
            now.saturating_add(self.policy.clock_skew_seconds) < not_before
        }) {
            return Err(AuthnError::InvalidToken);
        }
        if claims
            .iat
            .is_some_and(|issued_at| now.saturating_add(self.policy.clock_skew_seconds) < issued_at)
        {
            return Err(AuthnError::InvalidToken);
        }
        Ok(())
    }

    fn identity_from_claims(&self, claims: JwtClaims) -> Result<Identity, AuthnError> {
        let id = self.prefixed_subject(&claims.sub)?;
        let display = match claims.name {
            Some(display) if valid_claim_text(&display, 512) => Some(display),
            Some(_) => return Err(AuthnError::InvalidToken),
            None => None,
        };
        let kind = claims
            .schemahub_identity_kind
            .unwrap_or(IdentityKind::Human);
        let delegated_by = match claims.schemahub_delegated_by {
            Some(subject) => Some(self.prefixed_subject(&subject)?),
            None => None,
        };
        match kind {
            IdentityKind::Human if delegated_by.is_none() => Ok(match display {
                Some(display) => Identity::user_with_display(id, display),
                None => Identity::user(id),
            }),
            IdentityKind::Agent => Ok(Identity::agent(id, display, delegated_by)),
            IdentityKind::Service if delegated_by.is_none() => Ok(Identity::service(id, display)),
            IdentityKind::Anonymous | IdentityKind::Human | IdentityKind::Service => {
                Err(AuthnError::InvalidToken)
            }
        }
    }

    fn prefixed_subject(&self, subject: &str) -> Result<String, AuthnError> {
        if !valid_claim_text(subject, 512) {
            return Err(AuthnError::InvalidToken);
        }
        Ok(format!("{}{subject}", self.policy.identity_id_prefix))
    }
}

impl AuthnProvider for JwtAuthn {
    fn identify(&self, token: Option<&str>) -> Result<Identity, AuthnError> {
        match token.filter(|token| !token.is_empty()) {
            Some(token) => self.identify_token(token),
            None => Ok(Identity::Anonymous),
        }
    }
}

fn valid_claim_text(value: &str, max_bytes: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max_bytes && !value.chars().any(char::is_control)
}

#[derive(Debug, Deserialize, Serialize)]
struct JwtClaims {
    iss: String,
    sub: String,
    aud: JwtAudience,
    exp: u64,
    #[serde(default)]
    nbf: Option<u64>,
    #[serde(default)]
    iat: Option<u64>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    schemahub_identity_kind: Option<IdentityKind>,
    #[serde(default)]
    schemahub_delegated_by: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
enum JwtAudience {
    One(String),
    Many(Vec<String>),
}

fn validated_decoding_keys(
    jwks: &JwkSet,
    allowed_algorithms: &HashSet<Algorithm>,
) -> anyhow::Result<HashMap<(String, Algorithm), DecodingKey>> {
    let mut keys = HashMap::new();
    let mut key_ids = HashSet::new();
    for jwk in &jwks.keys {
        if !matches!(
            jwk.common.public_key_use,
            None | Some(PublicKeyUse::Signature)
        ) {
            continue;
        }
        if jwk
            .common
            .key_operations
            .as_ref()
            .is_some_and(|operations| !operations.contains(&KeyOperations::Verify))
        {
            continue;
        }
        let Some(configured_algorithm) = jwk.common.key_algorithm else {
            continue;
        };
        let Ok(algorithm) = configured_algorithm.to_string().parse::<Algorithm>() else {
            continue;
        };
        if !allowed_algorithms.contains(&algorithm) {
            continue;
        }
        let Some(kid) = jwk
            .common
            .key_id
            .as_ref()
            .filter(|kid| !kid.is_empty())
            .cloned()
        else {
            continue;
        };
        if !key_ids.insert(kid.clone()) {
            anyhow::bail!("JWKS contains duplicate signing key id {kid:?}");
        }
        let key = DecodingKey::from_jwk(jwk)
            .with_context(|| format!("building decoding key for JWKS kid {kid:?}"))?;
        if key.family() != algorithm.family() {
            anyhow::bail!(
                "JWKS key {kid:?} has algorithm {algorithm:?} but incompatible key material"
            );
        }
        keys.insert((kid, algorithm), key);
    }
    if keys.is_empty() {
        anyhow::bail!("JWKS contains no usable key for the configured algorithms");
    }
    Ok(keys)
}

#[derive(Clone)]
enum JwksLoader {
    File {
        path: PathBuf,
        max_bytes: usize,
    },
    Https {
        client: reqwest::Client,
        url: reqwest::Url,
        max_bytes: usize,
    },
}

impl JwksLoader {
    fn from_config(config: &JwtAuthConfig) -> anyhow::Result<Self> {
        if let Some(path) = &config.jwks_file {
            return Ok(Self::File {
                path: PathBuf::from(path),
                max_bytes: config.max_jwks_bytes,
            });
        }
        let url = config
            .jwks_url
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("JWT configuration has no JWKS source"))?
            .parse::<reqwest::Url>()?;
        let client = reqwest::Client::builder()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(config.request_timeout_seconds))
            .user_agent(format!("schemahub-server/{}", crate::BUILD_VERSION))
            .build()
            .context("building JWKS HTTP client")?;
        Ok(Self::Https {
            client,
            url,
            max_bytes: config.max_jwks_bytes,
        })
    }

    async fn load(&self) -> anyhow::Result<JwkSet> {
        let bytes = match self {
            Self::File { path, max_bytes } => {
                let metadata = tokio::fs::metadata(path)
                    .await
                    .with_context(|| format!("reading JWKS metadata from {}", path.display()))?;
                if metadata.len() > *max_bytes as u64 {
                    anyhow::bail!("JWKS file exceeds configured max_jwks_bytes");
                }
                let bytes = tokio::fs::read(path)
                    .await
                    .with_context(|| format!("reading JWKS file {}", path.display()))?;
                if bytes.len() > *max_bytes {
                    anyhow::bail!("JWKS file exceeds configured max_jwks_bytes");
                }
                bytes
            }
            Self::Https {
                client,
                url,
                max_bytes,
            } => {
                let mut response = client
                    .get(url.clone())
                    .header(ACCEPT, "application/json")
                    .send()
                    .await
                    .context("fetching configured JWKS URL")?
                    .error_for_status()
                    .context("configured JWKS URL returned an error status")?;
                if response
                    .content_length()
                    .is_some_and(|length| length > *max_bytes as u64)
                {
                    anyhow::bail!("remote JWKS exceeds configured max_jwks_bytes");
                }
                let mut bytes = Vec::new();
                while let Some(chunk) = response
                    .chunk()
                    .await
                    .context("streaming configured JWKS response")?
                {
                    if bytes.len().saturating_add(chunk.len()) > *max_bytes {
                        anyhow::bail!("remote JWKS exceeds configured max_jwks_bytes");
                    }
                    bytes.extend_from_slice(&chunk);
                }
                bytes
            }
        };
        serde_json::from_slice(&bytes).context("decoding JWKS JSON")
    }
}

/// Startup-loaded JWT verifier plus its supervised key-refresh loop.
pub struct JwtAuthRuntime {
    authn: Arc<JwtAuthn>,
    loader: JwksLoader,
    refresh_interval: Duration,
}

impl JwtAuthRuntime {
    pub async fn initialize(config: &JwtAuthConfig) -> anyhow::Result<Self> {
        Self::initialize_with_clock(config, Arc::new(SystemJwtClock)).await
    }

    /// Initialize with an explicit clock for deterministic embedding and
    /// acceptance tests. Production callers should use [`Self::initialize`].
    pub async fn initialize_with_clock(
        config: &JwtAuthConfig,
        clock: Arc<dyn JwtClock>,
    ) -> anyhow::Result<Self> {
        let loader = JwksLoader::from_config(config)?;
        let jwks = loader
            .load()
            .await
            .context("loading initial JWT verification keys")?;
        let authn = Arc::new(JwtAuthn::new(config, &jwks, clock)?);
        let key_count = authn.key_count()?;
        tracing::info!(
            event = "schemahub.auth.jwks_loaded",
            key_count,
            "initial JWT verification keys loaded"
        );
        Ok(Self {
            authn,
            loader,
            refresh_interval: Duration::from_secs(config.refresh_interval_seconds),
        })
    }

    pub fn provider(&self) -> Arc<dyn AuthnProvider> {
        self.authn.clone()
    }

    pub async fn run(
        self,
        mut shutdown: watch::Receiver<bool>,
        readiness: Readiness,
    ) -> anyhow::Result<&'static str> {
        let mut interval = tokio::time::interval(self.refresh_interval);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        interval.tick().await;
        let mut freshness_poll = tokio::time::interval(AUTH_FRESHNESS_POLL_INTERVAL);
        freshness_poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        freshness_poll.tick().await;
        let mut auth_ready = true;
        loop {
            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        return Ok("JWT JWKS refresh");
                    }
                }
                _ = interval.tick() => {
                    match self.loader.load().await.and_then(|jwks| self.authn.replace_jwks(&jwks)) {
                        Ok(key_count) => {
                            readiness.mark_auth_ready();
                            if !auth_ready {
                                tracing::info!(
                                    event = "schemahub.auth.jwks_freshness_changed",
                                    cache_fresh = true,
                                    "JWT verification key freshness recovered"
                                );
                            }
                            auth_ready = true;
                            tracing::info!(
                                event = "schemahub.auth.jwks_refreshed",
                                key_count,
                                "JWT verification keys refreshed"
                            );
                        }
                        Err(error) => {
                            let cache_fresh = self.authn.is_fresh().unwrap_or(false);
                            if !cache_fresh && auth_ready {
                                readiness.mark_auth_unready();
                                auth_ready = false;
                                tracing::warn!(
                                    event = "schemahub.auth.jwks_freshness_changed",
                                    cache_fresh = false,
                                    "JWT verification keys became stale"
                                );
                            }
                            tracing::warn!(
                                event = "schemahub.auth.jwks_refresh_failed",
                                cache_fresh,
                                error = %format!("{error:#}"),
                                "JWT verification key refresh failed"
                            );
                        }
                    }
                }
                _ = freshness_poll.tick() => {
                    let cache_fresh = self.authn.is_fresh().unwrap_or(false);
                    if cache_fresh != auth_ready {
                        if cache_fresh {
                            readiness.mark_auth_ready();
                        } else {
                            readiness.mark_auth_unready();
                        }
                        auth_ready = cache_fresh;
                        tracing::warn!(
                            event = "schemahub.auth.jwks_freshness_changed",
                            cache_fresh,
                            "JWT verification key freshness changed"
                        );
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use jsonwebtoken::{encode, EncodingKey, Header};
    use serde_json::json;

    use super::*;

    const TEST_PRIVATE_KEY_DER: &[u8] = &[
        0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04,
        0x20, 0x6a, 0xc3, 0xfd, 0xee, 0xee, 0x29, 0x8a, 0x92, 0x63, 0x8b, 0x70, 0x0c, 0x4b, 0x11,
        0x7c, 0xc3, 0x2e, 0x2d, 0x2a, 0xce, 0x0d, 0xfd, 0x78, 0x76, 0x94, 0xe2, 0x4c, 0xae, 0x8a,
        0xd5, 0x82, 0x34,
    ];
    const TEST_PUBLIC_KEY_X: &str = "2-Jj2UvNCvQiUPNYRgSi0cJSPiJI6Rs6D0UTeEpQVj8";

    #[derive(Debug)]
    struct FakeClock {
        now: AtomicU64,
    }

    impl FakeClock {
        fn new(now: u64) -> Self {
            Self {
                now: AtomicU64::new(now),
            }
        }

        fn set(&self, now: u64) {
            self.now.store(now, Ordering::Release);
        }
    }

    impl JwtClock for FakeClock {
        fn now_unix_seconds(&self) -> anyhow::Result<u64> {
            Ok(self.now.load(Ordering::Acquire))
        }
    }

    fn config() -> JwtAuthConfig {
        JwtAuthConfig {
            issuer: "https://identity.example.test".to_string(),
            audiences: vec!["schemahub".to_string()],
            algorithms: vec!["EdDSA".to_string()],
            token_type: "at+jwt".to_string(),
            identity_id_prefix: "oidc:".to_string(),
            jwks_url: None,
            jwks_file: Some("unused-in-verifier-tests.json".to_string()),
            clock_skew_seconds: 5,
            refresh_interval_seconds: 60,
            max_stale_seconds: 600,
            request_timeout_seconds: 5,
            max_token_bytes: 8_192,
            max_jwks_bytes: 65_536,
        }
    }

    fn jwks(kid: &str) -> JwkSet {
        serde_json::from_value(json!({
            "keys": [{
                "kty": "OKP",
                "use": "sig",
                "key_ops": ["verify"],
                "crv": "Ed25519",
                "x": TEST_PUBLIC_KEY_X,
                "kid": kid,
                "alg": "EdDSA"
            }]
        }))
        .expect("valid test JWKS")
    }

    fn claims(exp: u64) -> JwtClaims {
        JwtClaims {
            iss: "https://identity.example.test".to_string(),
            sub: "alice".to_string(),
            aud: JwtAudience::One("schemahub".to_string()),
            exp,
            nbf: Some(900),
            iat: Some(900),
            name: Some("Alice".to_string()),
            schemahub_identity_kind: None,
            schemahub_delegated_by: None,
        }
    }

    fn token(kid: &str, claims: &JwtClaims) -> String {
        let mut header = Header::new(Algorithm::EdDSA);
        header.typ = Some("at+jwt".to_string());
        header.kid = Some(kid.to_string());
        encode(
            &header,
            claims,
            &EncodingKey::from_ed_der(TEST_PRIVATE_KEY_DER),
        )
        .expect("sign test token")
    }

    #[test]
    fn valid_token_resolves_prefixed_human_identity() {
        // Arrange
        let clock = Arc::new(FakeClock::new(1_000));
        let authn = JwtAuthn::new(&config(), &jwks("key-1"), clock).expect("build authn");
        let token = token("key-1", &claims(2_000));

        // Act
        let identity = authn.identify(Some(&token)).expect("valid identity");

        // Assert
        assert_eq!(identity.id(), Some("oidc:alice"));
        assert_eq!(identity.display(), Some("Alice"));
        assert_eq!(identity.kind(), IdentityKind::Human);
    }

    #[test]
    fn trusted_agent_claim_preserves_kind_and_prefixed_delegation() {
        // Arrange
        let clock = Arc::new(FakeClock::new(1_000));
        let authn = JwtAuthn::new(&config(), &jwks("key-1"), clock).expect("build authn");
        let mut agent_claims = claims(2_000);
        agent_claims.sub = "schema-agent".to_string();
        agent_claims.schemahub_identity_kind = Some(IdentityKind::Agent);
        agent_claims.schemahub_delegated_by = Some("alice".to_string());
        let token = token("key-1", &agent_claims);

        // Act
        let identity = authn.identify(Some(&token)).expect("valid identity");

        // Assert
        assert_eq!(identity.id(), Some("oidc:schema-agent"));
        assert_eq!(identity.kind(), IdentityKind::Agent);
        assert_eq!(identity.delegated_by(), Some("oidc:alice"));
    }

    #[test]
    fn expired_token_is_rejected_using_injected_time() {
        // Arrange
        let clock = Arc::new(FakeClock::new(1_006));
        let authn = JwtAuthn::new(&config(), &jwks("key-1"), clock).expect("build authn");
        let token = token("key-1", &claims(1_001));

        // Act
        let result = authn.identify(Some(&token));

        // Assert
        assert!(matches!(result, Err(AuthnError::InvalidToken)));
    }

    #[test]
    fn wrong_audience_is_rejected() {
        // Arrange
        let clock = Arc::new(FakeClock::new(1_000));
        let authn = JwtAuthn::new(&config(), &jwks("key-1"), clock).expect("build authn");
        let mut wrong_audience = claims(2_000);
        wrong_audience.aud = JwtAudience::One("another-service".to_string());
        let token = token("key-1", &wrong_audience);

        // Act
        let result = authn.identify(Some(&token));

        // Assert
        assert!(matches!(result, Err(AuthnError::InvalidToken)));
    }

    #[test]
    fn key_rotation_atomically_replaces_the_previous_key_id() {
        // Arrange
        let clock = Arc::new(FakeClock::new(1_000));
        let authn = JwtAuthn::new(&config(), &jwks("key-1"), clock).expect("build authn");
        let old_token = token("key-1", &claims(2_000));
        let new_token = token("key-2", &claims(2_000));

        // Act
        let count = authn.replace_jwks(&jwks("key-2")).expect("rotate keys");

        // Assert
        assert_eq!(count, 1);
        assert!(matches!(
            authn.identify(Some(&old_token)),
            Err(AuthnError::InvalidToken)
        ));
        assert_eq!(
            authn
                .identify(Some(&new_token))
                .expect("new key works")
                .id(),
            Some("oidc:alice")
        );
    }

    #[test]
    fn stale_key_cache_fails_closed() {
        // Arrange
        let clock = Arc::new(FakeClock::new(1_000));
        let authn = JwtAuthn::new(&config(), &jwks("key-1"), clock.clone()).expect("build authn");
        let token = token("key-1", &claims(3_000));
        clock.set(1_601);

        // Act
        let result = authn.identify(Some(&token));

        // Assert
        assert!(matches!(result, Err(AuthnError::Other(_))));
    }

    #[test]
    fn malformed_refresh_preserves_last_known_good_keys() {
        // Arrange
        let clock = Arc::new(FakeClock::new(1_000));
        let authn = JwtAuthn::new(&config(), &jwks("key-1"), clock).expect("build authn");
        let token = token("key-1", &claims(2_000));
        let empty = JwkSet { keys: Vec::new() };

        // Act
        let refresh = authn.replace_jwks(&empty);

        // Assert
        assert!(refresh.is_err());
        assert_eq!(
            authn.identify(Some(&token)).expect("old key retained").id(),
            Some("oidc:alice")
        );
    }

    #[test]
    fn absent_token_remains_anonymous_for_public_reads() {
        // Arrange
        let clock = Arc::new(FakeClock::new(1_000));
        let authn = JwtAuthn::new(&config(), &jwks("key-1"), clock).expect("build authn");

        // Act
        let identity = authn.identify(None).expect("anonymous identity");

        // Assert
        assert!(identity.is_anonymous());
    }
}
