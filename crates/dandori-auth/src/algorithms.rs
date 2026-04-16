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
    parse_allowed_algorithms_with_profile(value, current_profile())
}

fn parse_allowed_algorithms_with_profile(
    value: &str,
    profile: AuthProfile,
) -> Result<Vec<Algorithm>, AuthError> {
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
            "HS256" | "HS384" | "HS512" if profile == AuthProfile::Dev => match item {
                "HS256" => Algorithm::HS256,
                "HS384" => Algorithm::HS384,
                _ => Algorithm::HS512,
            },
            "HS256" | "HS384" | "HS512" => return Err(AuthError::AlgorithmMismatch),
            _ => return Err(AuthError::AlgorithmMismatch),
        };
        parsed.push(algorithm);
    }

    if parsed.is_empty() {
        return Err(AuthError::AlgorithmMismatch);
    }
    Ok(parsed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AuthProfile {
    Dev,
    Prod,
}

/// Resolve the auth profile from `DANDORI_PROFILE`. In dev builds (debug
/// assertions enabled) the profile defaults to Dev so tests keep working; in
/// release builds the default is Prod, and only an explicit `dev` value
/// flips it. This makes accidentally allowing HS* in a release binary
/// impossible without an intentional env override.
fn current_profile() -> AuthProfile {
    match std::env::var("DANDORI_PROFILE").as_deref() {
        Ok("dev") => AuthProfile::Dev,
        Ok(_) => AuthProfile::Prod,
        Err(_) => {
            if cfg!(debug_assertions) {
                AuthProfile::Dev
            } else {
                AuthProfile::Prod
            }
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hs_rejected_in_prod_profile() {
        let result = parse_allowed_algorithms_with_profile("HS256", AuthProfile::Prod);
        assert!(
            result.is_err(),
            "HS256 must be rejected outside dev profile"
        );
    }

    #[test]
    fn hs_accepted_in_dev_profile() {
        let result = parse_allowed_algorithms_with_profile("HS256,HS384", AuthProfile::Dev)
            .expect("HS should be allowed in dev");
        assert_eq!(result, vec![Algorithm::HS256, Algorithm::HS384]);
    }

    #[test]
    fn rs_accepted_in_both_profiles() {
        for profile in [AuthProfile::Dev, AuthProfile::Prod] {
            let result = parse_allowed_algorithms_with_profile("RS256,ES256", profile)
                .expect("asymmetric algs always allowed");
            assert_eq!(result, vec![Algorithm::RS256, Algorithm::ES256]);
        }
    }

    #[test]
    fn mixed_hs_and_rs_rejected_in_prod() {
        let result = parse_allowed_algorithms_with_profile("RS256,HS256", AuthProfile::Prod);
        assert!(result.is_err());
    }
}
