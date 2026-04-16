use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Duration;

mod algorithms;

use jsonwebtoken::{
    Algorithm, DecodingKey, TokenData, Validation, decode, decode_header,
    jwk::{Jwk, JwkSet},
};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use thiserror::Error;
use tracing::{info, warn};
use uuid::Uuid;

use crate::algorithms::{
    default_runtime_allowed_algorithms, default_test_allowed_algorithms, map_key_algorithm,
    parse_allowed_algorithms,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthenticatedClaims {
    pub actor_id: Uuid,
    pub workspace_id: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OidcConfig {
    pub issuer: String,
    pub audience: String,
    pub jwks_source: JwksSource,
    pub allowed_algorithms: Vec<Algorithm>,
    pub jwks_refresh: JwksRefreshConfig,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JwksRefreshConfig {
    pub interval_millis: u64,
    pub timeout_millis: u64,
    pub max_backoff_millis: u64,
}

impl Default for JwksRefreshConfig {
    fn default() -> Self {
        Self {
            interval_millis: 300_000,
            timeout_millis: 2_000,
            max_backoff_millis: 300_000,
        }
    }
}

impl JwksRefreshConfig {
    fn interval(&self) -> Duration {
        Duration::from_millis(self.interval_millis.max(1))
    }

    fn timeout(&self) -> Duration {
        Duration::from_millis(self.timeout_millis.max(1))
    }

    fn max_backoff(&self) -> Duration {
        Duration::from_millis(self.max_backoff_millis.max(self.interval_millis.max(1)))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JwksSource {
    Path(PathBuf),
    Url(String),
}

#[derive(Clone)]
pub struct JwtAuthenticator {
    issuer: String,
    audience: String,
    allowed_algorithms: Vec<Algorithm>,
    keyset: Arc<RwLock<JwkKeyset>>,
}

#[derive(Clone)]
struct JwkEntry {
    decoding_key: DecodingKey,
    algorithm: Option<Algorithm>,
}

#[derive(Clone, Default)]
struct JwkKeyset {
    keys: HashMap<String, JwkEntry>,
    fallback_key: Option<JwkEntry>,
}

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("missing required environment variable '{0}'")]
    MissingEnv(&'static str),
    #[error("exactly one of DANDORI_OIDC_JWKS_PATH or DANDORI_OIDC_JWKS_URL must be configured")]
    InvalidJwksSource,
    #[error("failed to parse environment variable '{name}' as a positive integer")]
    InvalidEnvNumber { name: &'static str },
    #[error("failed to read JWKS file '{path}': {source}")]
    ReadJwksPath {
        path: String,
        source: std::io::Error,
    },
    #[error("failed to fetch JWKS URL '{url}': {source}")]
    FetchJwksUrl { url: String, source: reqwest::Error },
    #[error("invalid JWKS document: {0}")]
    InvalidJwks(serde_json::Error),
    #[error("JWKS has no keys")]
    EmptyJwks,
    #[error("failed to build decoding key from JWK: {0}")]
    BuildDecodingKey(jsonwebtoken::errors::Error),
    #[error("token header decode failed: {0}")]
    TokenHeader(jsonwebtoken::errors::Error),
    #[error("token missing kid header with multiple configured JWKS keys")]
    MissingKid,
    #[error("no configured key for token kid '{0}'")]
    UnknownKid(String),
    #[error("token algorithm is not allowed for selected key")]
    AlgorithmMismatch,
    #[error("token algorithm '{0}' is not in allowed algorithms")]
    DisallowedAlgorithm(String),
    #[error("token verification failed: {0}")]
    TokenVerify(jsonwebtoken::errors::Error),
    #[error("invalid 'sub' claim as UUID: {0}")]
    InvalidSub(uuid::Error),
    #[error("invalid 'workspace_id' claim as UUID: {0}")]
    InvalidWorkspaceId(uuid::Error),
}

#[derive(Debug, Clone, Deserialize)]
struct JwtClaims {
    sub: String,
    workspace_id: String,
}

impl JwtAuthenticator {
    pub async fn from_env() -> Result<Self, AuthError> {
        let config = OidcConfig::from_env()?;
        Self::from_config(config).await
    }

    pub async fn from_config(config: OidcConfig) -> Result<Self, AuthError> {
        let jwks_raw = load_jwks_raw(&config.jwks_source).await?;
        let keyset = parse_jwks(&jwks_raw)?;

        let authenticator = Self {
            issuer: config.issuer,
            audience: config.audience,
            allowed_algorithms: config.allowed_algorithms,
            keyset: Arc::new(RwLock::new(keyset)),
        };

        authenticator.start_refresh_task(config.jwks_source, config.jwks_refresh);

        Ok(authenticator)
    }

    pub fn from_jwks_json(
        issuer: String,
        audience: String,
        jwks_raw: &str,
    ) -> Result<Self, AuthError> {
        Self::from_jwks_json_with_allowed_algorithms(
            issuer,
            audience,
            jwks_raw,
            default_test_allowed_algorithms(),
        )
    }

    pub fn from_jwks_json_with_allowed_algorithms(
        issuer: String,
        audience: String,
        jwks_raw: &str,
        allowed_algorithms: Vec<Algorithm>,
    ) -> Result<Self, AuthError> {
        let keyset = parse_jwks(jwks_raw)?;

        Ok(Self {
            issuer,
            audience,
            allowed_algorithms,
            keyset: Arc::new(RwLock::new(keyset)),
        })
    }

    pub fn authenticate_token(
        &self,
        token: &SecretString,
    ) -> Result<AuthenticatedClaims, AuthError> {
        let header = decode_header(token.expose_secret()).map_err(AuthError::TokenHeader)?;

        let keyset = match self.keyset.read() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };

        let selected_key = if let Some(kid) = header.kid {
            keyset
                .keys
                .get(&kid)
                .cloned()
                .ok_or(AuthError::UnknownKid(kid))?
        } else if keyset.keys.len() <= 1 {
            if let Some(entry) = keyset.keys.values().next() {
                entry.clone()
            } else {
                keyset.fallback_key.clone().ok_or(AuthError::MissingKid)?
            }
        } else {
            return Err(AuthError::MissingKid);
        };

        if let Some(expected_algorithm) = selected_key.algorithm {
            if expected_algorithm != header.alg {
                return Err(AuthError::AlgorithmMismatch);
            }
        }

        if !self.allowed_algorithms.contains(&header.alg) {
            return Err(AuthError::DisallowedAlgorithm(format!("{:?}", header.alg)));
        }

        let mut validation = Validation::new(header.alg);
        validation.set_issuer(&[self.issuer.as_str()]);
        validation.set_audience(&[self.audience.as_str()]);

        let token_data: TokenData<JwtClaims> = decode(
            token.expose_secret(),
            &selected_key.decoding_key,
            &validation,
        )
        .map_err(AuthError::TokenVerify)?;

        let actor_id =
            Uuid::parse_str(token_data.claims.sub.as_str()).map_err(AuthError::InvalidSub)?;
        let workspace_id = Uuid::parse_str(token_data.claims.workspace_id.as_str())
            .map_err(AuthError::InvalidWorkspaceId)?;

        Ok(AuthenticatedClaims {
            actor_id,
            workspace_id,
        })
    }

    fn start_refresh_task(&self, source: JwksSource, refresh: JwksRefreshConfig) {
        let keyset = Arc::clone(&self.keyset);
        tokio::spawn(async move {
            let mut next_delay = refresh.interval();
            loop {
                tokio::time::sleep(next_delay).await;
                let result = tokio::time::timeout(refresh.timeout(), load_jwks_raw(&source)).await;

                let new_keyset = match result {
                    Ok(Ok(raw)) => parse_jwks(&raw),
                    Ok(Err(error)) => Err(error),
                    Err(_) => {
                        warn!(
                            source = ?source,
                            timeout_millis = refresh.timeout_millis,
                            "jwks refresh timed out"
                        );
                        next_delay = next_delay.saturating_mul(2).min(refresh.max_backoff());
                        continue;
                    }
                };

                match new_keyset {
                    Ok(parsed) => {
                        let keys_count = parsed.keys.len();
                        {
                            let mut guard = match keyset.write() {
                                Ok(guard) => guard,
                                Err(poisoned) => poisoned.into_inner(),
                            };
                            *guard = parsed;
                        }
                        info!(source = ?source, keys_count, "jwks refresh succeeded");
                        next_delay = refresh.interval();
                    }
                    Err(error) => {
                        warn!(source = ?source, error = %error, "jwks refresh failed");
                        next_delay = next_delay.saturating_mul(2).min(refresh.max_backoff());
                    }
                }
            }
        });
    }
}

impl std::fmt::Debug for JwtAuthenticator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let keys_count = match self.keyset.read() {
            Ok(guard) => guard.keys.len(),
            Err(poisoned) => poisoned.into_inner().keys.len(),
        };

        f.debug_struct("JwtAuthenticator")
            .field("issuer", &self.issuer)
            .field("audience", &self.audience)
            .field("allowed_algorithms", &self.allowed_algorithms)
            .field("keys_count", &keys_count)
            .finish_non_exhaustive()
    }
}

impl OidcConfig {
    pub fn from_env() -> Result<Self, AuthError> {
        let issuer = std::env::var("DANDORI_OIDC_ISSUER")
            .map_err(|_| AuthError::MissingEnv("DANDORI_OIDC_ISSUER"))?;
        let audience = std::env::var("DANDORI_OIDC_AUDIENCE")
            .map_err(|_| AuthError::MissingEnv("DANDORI_OIDC_AUDIENCE"))?;

        let path = std::env::var("DANDORI_OIDC_JWKS_PATH").ok();
        let url = std::env::var("DANDORI_OIDC_JWKS_URL").ok();

        let jwks_source = match (path, url) {
            (Some(path), None) => JwksSource::Path(PathBuf::from(path)),
            (None, Some(url)) => JwksSource::Url(url),
            _ => return Err(AuthError::InvalidJwksSource),
        };

        let allowed_algorithms = std::env::var("DANDORI_OIDC_ALLOWED_ALGS")
            .ok()
            .map(|value| parse_allowed_algorithms(value.as_str()))
            .transpose()?
            .unwrap_or_else(default_runtime_allowed_algorithms);

        let jwks_refresh = JwksRefreshConfig {
            interval_millis: parse_env_u64(
                "DANDORI_OIDC_JWKS_REFRESH_INTERVAL_MILLIS",
                JwksRefreshConfig::default().interval_millis,
            )?,
            timeout_millis: parse_env_u64(
                "DANDORI_OIDC_JWKS_REFRESH_TIMEOUT_MILLIS",
                JwksRefreshConfig::default().timeout_millis,
            )?,
            max_backoff_millis: parse_env_u64(
                "DANDORI_OIDC_JWKS_REFRESH_MAX_BACKOFF_MILLIS",
                JwksRefreshConfig::default().max_backoff_millis,
            )?,
        };

        Ok(Self {
            issuer,
            audience,
            jwks_source,
            allowed_algorithms,
            jwks_refresh,
        })
    }
}

impl JwkEntry {
    fn try_from_jwk(jwk: &Jwk) -> Result<Self, AuthError> {
        let decoding_key = DecodingKey::from_jwk(jwk).map_err(AuthError::BuildDecodingKey)?;
        let algorithm = jwk
            .common
            .key_algorithm
            .as_ref()
            .map(map_key_algorithm)
            .transpose()?;

        Ok(Self {
            decoding_key,
            algorithm,
        })
    }
}

fn parse_env_u64(name: &'static str, default: u64) -> Result<u64, AuthError> {
    let Some(raw) = std::env::var(name).ok() else {
        return Ok(default);
    };

    raw.parse::<u64>()
        .map_err(|_| AuthError::InvalidEnvNumber { name })
}

async fn load_jwks_raw(source: &JwksSource) -> Result<String, AuthError> {
    match source {
        JwksSource::Path(path) => {
            std::fs::read_to_string(path).map_err(|source| AuthError::ReadJwksPath {
                path: path.display().to_string(),
                source,
            })
        }
        JwksSource::Url(url) => reqwest::get(url)
            .await
            .map_err(|source| AuthError::FetchJwksUrl {
                url: url.clone(),
                source,
            })?
            .error_for_status()
            .map_err(|source| AuthError::FetchJwksUrl {
                url: url.clone(),
                source,
            })?
            .text()
            .await
            .map_err(|source| AuthError::FetchJwksUrl {
                url: url.clone(),
                source,
            }),
    }
}

fn parse_jwks(jwks_raw: &str) -> Result<JwkKeyset, AuthError> {
    let jwk_set: JwkSet = serde_json::from_str(jwks_raw).map_err(AuthError::InvalidJwks)?;
    if jwk_set.keys.is_empty() {
        return Err(AuthError::EmptyJwks);
    }

    let mut keys = HashMap::new();
    let mut fallback_key = None;

    for jwk in &jwk_set.keys {
        let entry = JwkEntry::try_from_jwk(jwk)?;
        if let Some(key_id) = &jwk.common.key_id {
            keys.insert(key_id.clone(), entry.clone());
        } else if fallback_key.is_none() {
            fallback_key = Some(entry.clone());
        }
    }

    if keys.is_empty() && fallback_key.is_none() {
        return Err(AuthError::EmptyJwks);
    }

    Ok(JwkKeyset { keys, fallback_key })
}

#[cfg(test)]
mod tests;
