use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};

use crate::config::AuthConfig;

pub fn check_api_key(headers: &HeaderMap, auth: Option<&AuthConfig>) -> Result<(), Box<Response>> {
    let Some(auth) = auth else {
        return Ok(());
    };
    if auth.api_keys.is_empty() {
        return Ok(());
    }

    let header = headers
        .get("x-api-key")
        .or_else(|| headers.get("authorization"));
    let Some(value) = header else {
        return Err(Box::new(
            (StatusCode::UNAUTHORIZED, "missing api key").into_response(),
        ));
    };
    let Ok(value) = value.to_str() else {
        return Err(Box::new(
            (StatusCode::UNAUTHORIZED, "invalid api key").into_response(),
        ));
    };
    let key = value.strip_prefix("Bearer ").unwrap_or(value);
    if auth.api_keys.iter().any(|allowed| allowed == key) {
        return Ok(());
    }
    Err(Box::new(
        (StatusCode::UNAUTHORIZED, "invalid api key").into_response(),
    ))
}
