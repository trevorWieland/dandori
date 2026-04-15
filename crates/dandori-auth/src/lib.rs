use std::collections::HashMap;
use std::path::PathBuf;

use jsonwebtoken::{
    Algorithm, DecodingKey, TokenData, Validation, decode, decode_header,
    jwk::{Jwk, JwkSet, KeyAlgorithm},
};
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use thiserror::Error;
use uuid::Uuid;

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
    keys: HashMap<String, JwkEntry>,
    fallback_key: Option<JwkEntry>,
}

#[derive(Clone)]
struct JwkEntry {
    decoding_key: DecodingKey,
    algorithm: Option<Algorithm>,
}

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("missing required environment variable '{0}'")]
    MissingEnv(&'static str),
    #[error("exactly one of DANDORI_OIDC_JWKS_PATH or DANDORI_OIDC_JWKS_URL must be configured")]
    InvalidJwksSource,
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
        let jwks_raw = match &config.jwks_source {
            JwksSource::Path(path) => {
                std::fs::read_to_string(path).map_err(|source| AuthError::ReadJwksPath {
                    path: path.display().to_string(),
                    source,
                })?
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
                })?,
        };

        Self::from_jwks_json_with_allowed_algorithms(
            config.issuer,
            config.audience,
            &jwks_raw,
            config.allowed_algorithms,
        )
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

        Ok(Self {
            issuer,
            audience,
            allowed_algorithms,
            keys,
            fallback_key,
        })
    }

    pub fn authenticate_token(
        &self,
        token: &SecretString,
    ) -> Result<AuthenticatedClaims, AuthError> {
        let header = decode_header(token.expose_secret()).map_err(AuthError::TokenHeader)?;
        let selected_key = if let Some(kid) = header.kid {
            self.keys
                .get(&kid)
                .cloned()
                .ok_or(AuthError::UnknownKid(kid))?
        } else if self.keys.len() <= 1 {
            if let Some(entry) = self.keys.values().next() {
                entry.clone()
            } else {
                self.fallback_key.clone().ok_or(AuthError::MissingKid)?
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
}

impl std::fmt::Debug for JwtAuthenticator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JwtAuthenticator")
            .field("issuer", &self.issuer)
            .field("audience", &self.audience)
            .field("allowed_algorithms", &self.allowed_algorithms)
            .field("keys_count", &self.keys.len())
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

        Ok(Self {
            issuer,
            audience,
            jwks_source,
            allowed_algorithms,
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

fn map_key_algorithm(algorithm: &KeyAlgorithm) -> Result<Algorithm, AuthError> {
    let mapped = match algorithm {
        KeyAlgorithm::HS256 => Algorithm::HS256,
        KeyAlgorithm::HS384 => Algorithm::HS384,
        KeyAlgorithm::HS512 => Algorithm::HS512,
        KeyAlgorithm::ES256 => Algorithm::ES256,
        KeyAlgorithm::ES384 => Algorithm::ES384,
        KeyAlgorithm::RS256 => Algorithm::RS256,
        KeyAlgorithm::RS384 => Algorithm::RS384,
        KeyAlgorithm::RS512 => Algorithm::RS512,
        KeyAlgorithm::PS256 => Algorithm::PS256,
        KeyAlgorithm::PS384 => Algorithm::PS384,
        KeyAlgorithm::PS512 => Algorithm::PS512,
        KeyAlgorithm::EdDSA => Algorithm::EdDSA,
        _ => return Err(AuthError::AlgorithmMismatch),
    };

    Ok(mapped)
}

fn parse_allowed_algorithms(value: &str) -> Result<Vec<Algorithm>, AuthError> {
    let mut parsed = Vec::new();
    for raw in value.split(',') {
        let item = raw.trim();
        if item.is_empty() {
            continue;
        }

        let algorithm = match item {
            "RS256" => Algorithm::RS256,
            "RS384" => Algorithm::RS384,
            "RS512" => Algorithm::RS512,
            "PS256" => Algorithm::PS256,
            "PS384" => Algorithm::PS384,
            "PS512" => Algorithm::PS512,
            "ES256" => Algorithm::ES256,
            "ES384" => Algorithm::ES384,
            "EdDSA" => Algorithm::EdDSA,
            "HS256" => Algorithm::HS256,
            "HS384" => Algorithm::HS384,
            "HS512" => Algorithm::HS512,
            _ => return Err(AuthError::AlgorithmMismatch),
        };
        parsed.push(algorithm);
    }

    if parsed.is_empty() {
        return Err(AuthError::AlgorithmMismatch);
    }
    Ok(parsed)
}

fn default_runtime_allowed_algorithms() -> Vec<Algorithm> {
    vec![
        Algorithm::RS256,
        Algorithm::RS384,
        Algorithm::RS512,
        Algorithm::PS256,
        Algorithm::PS384,
        Algorithm::PS512,
        Algorithm::ES256,
        Algorithm::ES384,
        Algorithm::EdDSA,
    ]
}

fn default_test_allowed_algorithms() -> Vec<Algorithm> {
    vec![
        Algorithm::HS256,
        Algorithm::HS384,
        Algorithm::HS512,
        Algorithm::RS256,
        Algorithm::RS384,
        Algorithm::RS512,
        Algorithm::PS256,
        Algorithm::PS384,
        Algorithm::PS512,
        Algorithm::ES256,
        Algorithm::ES384,
        Algorithm::EdDSA,
    ]
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use jsonwebtoken::{EncodingKey, Header, encode};

    use super::*;

    const TEST_JWKS: &str =
        r#"{"keys":[{"kty":"oct","k":"c2VjcmV0","alg":"HS256","kid":"test-key"}]}"#;

    #[derive(Debug, serde::Serialize)]
    struct Claims {
        sub: String,
        workspace_id: String,
        iss: String,
        aud: String,
        exp: usize,
        nbf: usize,
    }

    fn build_token(issuer: &str, audience: &str, workspace_id: Uuid) -> SecretString {
        let mut header = Header::new(Algorithm::HS256);
        header.kid = Some("test-key".to_owned());

        let now = Utc::now().timestamp() as usize;
        let claims = Claims {
            sub: Uuid::now_v7().to_string(),
            workspace_id: workspace_id.to_string(),
            iss: issuer.to_owned(),
            aud: audience.to_owned(),
            exp: now + 3600,
            nbf: now.saturating_sub(30),
        };

        let token = encode(&header, &claims, &EncodingKey::from_secret(b"secret"))
            .expect("encode test token");
        SecretString::from(token)
    }

    #[test]
    fn validates_claims_from_jwks() {
        let issuer = "https://issuer.example".to_owned();
        let audience = "dandori".to_owned();
        let authenticator =
            JwtAuthenticator::from_jwks_json(issuer.clone(), audience.clone(), TEST_JWKS)
                .expect("authenticator");

        let workspace_id = Uuid::now_v7();
        let token = build_token(&issuer, &audience, workspace_id);
        let claims = authenticator.authenticate_token(&token).expect("claims");

        assert_eq!(claims.workspace_id, workspace_id);
    }

    #[test]
    fn rejects_wrong_issuer() {
        let authenticator = JwtAuthenticator::from_jwks_json(
            "https://issuer.example".to_owned(),
            "dandori".to_owned(),
            TEST_JWKS,
        )
        .expect("authenticator");

        let token = build_token("https://wrong.example", "dandori", Uuid::now_v7());
        let error = authenticator
            .authenticate_token(&token)
            .expect_err("issuer mismatch should fail");

        assert!(matches!(error, AuthError::TokenVerify(_)));
    }
}
