use jsonwebtoken::{Algorithm, jwk::KeyAlgorithm};

use crate::AuthError;

pub(super) fn map_key_algorithm(algorithm: &KeyAlgorithm) -> Result<Algorithm, AuthError> {
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

pub(super) fn parse_allowed_algorithms(value: &str) -> Result<Vec<Algorithm>, AuthError> {
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

pub(super) fn default_runtime_allowed_algorithms() -> Vec<Algorithm> {
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

pub(super) fn default_test_allowed_algorithms() -> Vec<Algorithm> {
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
