use chrono::Utc;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use secrecy::SecretString;
use uuid::Uuid;

use super::{AuthError, JwksRefreshConfig, JwksSource, JwtAuthenticator, OidcConfig};

const TEST_JWKS: &str = r#"{"keys":[{"kty":"oct","k":"c2VjcmV0","alg":"HS256","kid":"test-key"}]}"#;

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
    build_token_with_secret(issuer, audience, workspace_id, "test-key", b"secret")
}

fn build_token_with_secret(
    issuer: &str,
    audience: &str,
    workspace_id: Uuid,
    kid: &str,
    secret: &[u8],
) -> SecretString {
    let mut header = Header::new(Algorithm::HS256);
    header.kid = Some(kid.to_owned());

    let now = Utc::now().timestamp() as usize;
    let claims = Claims {
        sub: Uuid::now_v7().to_string(),
        workspace_id: workspace_id.to_string(),
        iss: issuer.to_owned(),
        aud: audience.to_owned(),
        exp: now + 3600,
        nbf: now.saturating_sub(30),
    };

    let token =
        encode(&header, &claims, &EncodingKey::from_secret(secret)).expect("encode test token");
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

#[tokio::test]
async fn refreshes_jwks_from_file_without_restart() {
    let temp = std::env::temp_dir().join(format!("dandori-auth-{}.json", Uuid::now_v7()));
    std::fs::write(
        &temp,
        r#"{"keys":[{"kty":"oct","k":"c2VjcmV0MQ","alg":"HS256","kid":"kid-1"}]}"#,
    )
    .expect("write initial jwks");

    let config = OidcConfig {
        issuer: "https://issuer.example".to_owned(),
        audience: "dandori".to_owned(),
        jwks_source: JwksSource::Path(temp.clone()),
        allowed_algorithms: vec![Algorithm::HS256],
        jwks_refresh: JwksRefreshConfig {
            interval_millis: 50,
            timeout_millis: 200,
            max_backoff_millis: 200,
        },
    };

    let authenticator = JwtAuthenticator::from_config(config)
        .await
        .expect("authenticator");

    let workspace_id = Uuid::now_v7();
    let first_token = build_token_with_secret(
        "https://issuer.example",
        "dandori",
        workspace_id,
        "kid-1",
        b"secret1",
    );
    authenticator
        .authenticate_token(&first_token)
        .expect("initial key should authenticate");

    std::fs::write(
        &temp,
        r#"{"keys":[{"kty":"oct","k":"c2VjcmV0Mg","alg":"HS256","kid":"kid-2"}]}"#,
    )
    .expect("rotate jwks");

    let second_token = build_token_with_secret(
        "https://issuer.example",
        "dandori",
        workspace_id,
        "kid-2",
        b"secret2",
    );

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        if authenticator.authenticate_token(&second_token).is_ok() {
            break;
        }

        assert!(
            std::time::Instant::now() < deadline,
            "jwks refresh did not pick up rotated key within timeout"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    let _ = std::fs::remove_file(temp);
}
