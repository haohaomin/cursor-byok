//! Bounds buffering at both sides of HTTP request decompression.
use axum::{
    body::{to_bytes, Body},
    extract::Request,
    http::{header, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use http_body_util::LengthLimitError;

// Covers large image/history appends, including hex's 2x wire overhead. Applies
// to locally handled unary requests, never Tab/file uploads or streaming relays.
const MAX_BODY_BYTES: usize = 64 * 1024 * 1024;

pub(super) async fn buffer(request: Request, next: Next) -> Response {
    if request
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .is_some_and(|length| length > MAX_BODY_BYTES as u64)
    {
        return too_large();
    }
    let (parts, body) = request.into_parts();
    match to_bytes(body, MAX_BODY_BYTES).await {
        Ok(bytes) => {
            next.run(Request::from_parts(parts, Body::from(bytes)))
                .await
        }
        Err(error)
            if std::error::Error::source(&error)
                .is_some_and(|source| source.is::<LengthLimitError>()) =>
        {
            too_large()
        }
        Err(error) => {
            crate::Error::Protocol(format!("cannot read request body: {error}")).into_response()
        }
    }
}

fn too_large() -> Response {
    tracing::warn!(
        limit_bytes = MAX_BODY_BYTES,
        "Cursor request body exceeds size limit"
    );
    // Connect clients read the explicit code; its prescribed HTTP status is 429.
    (StatusCode::TOO_MANY_REQUESTS, Json(serde_json::json!({
        "code": "resource_exhausted",
        "message": "Cursor request body exceeds the 64 MiB limit (before or after decompression); reduce the attached content or conversation history"
    }))).into_response()
}
