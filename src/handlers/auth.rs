use async_trait::async_trait;
use axum::{
    extract::FromRequestParts,
    http::{header, request::Parts, StatusCode},
};

#[derive(Clone)]
pub struct AuthToken(pub String);

#[async_trait]
impl<S> FromRequestParts<S> for AuthToken
where
    S: Send + Sync,
{
    type Rejection = (StatusCode, &'static str);

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let auth_header = parts
            .headers
            .get(header::AUTHORIZATION)
            .ok_or((StatusCode::UNAUTHORIZED, "Missing authorization header"))?;

        let token = auth_header
            .to_str()
            .unwrap_or("")
            .trim_start_matches("Bearer ")
            .to_string();

        if token.is_empty() {
            return Err((StatusCode::UNAUTHORIZED, "Invalid authorization token"));
        }

        Ok(AuthToken(token))
    }
}
