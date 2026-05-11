//! axum `IntoResponse` wrappers around `crabcloud_core::Error`. Two flavors:
//!
//! - `ApiError` — plain status + JSON body. For non-OCS endpoints.
//! - `OcsError` — wraps in the OCS envelope so `/ocs/*` responses match
//!   Nextcloud's wire format.

use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use crabcloud_core::Error as CoreError;
use crabcloud_ocs::{render, Format, OcsResponse, OcsStatus, OcsVersion};
use serde_json::json;

/// Plain HTTP error response. Body is JSON `{"error": "..."}` with the
/// `client_message()` text.
#[derive(Debug)]
pub struct ApiError(pub CoreError);

impl From<CoreError> for ApiError {
    fn from(e: CoreError) -> Self {
        Self(e)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status =
            StatusCode::from_u16(self.0.http_status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let body = Json(json!({ "error": self.0.client_message() }));
        (status, body).into_response()
    }
}

/// OCS-envelope error response. Wraps a `CoreError` into an `OcsResponse`
/// rendered as XML by default (or JSON via the `?format=json` query / Accept).
#[derive(Debug)]
pub struct OcsError {
    /// Underlying core error.
    pub error: CoreError,
    /// OCS protocol version to use when rendering.
    pub version: OcsVersion,
    /// Output format (XML/JSON).
    pub format: Format,
}

impl OcsError {
    /// Build an `OcsError` wrapping `error` with the requested version + format.
    pub fn new(error: CoreError, version: OcsVersion, format: Format) -> Self {
        Self {
            error,
            version,
            format,
        }
    }

    fn ocs_status(&self) -> OcsStatus {
        match self.error {
            CoreError::NotFound => OcsStatus::NotFound,
            CoreError::Unauthorized => OcsStatus::Unauthorized,
            CoreError::Forbidden => OcsStatus::Forbidden,
            CoreError::BadRequest(_) => OcsStatus::BadRequest,
            CoreError::Conflict(_) => OcsStatus::BadRequest, // OCS has no 409
            CoreError::Locked => OcsStatus::ServerError,     // 423 not in OCS palette
            CoreError::Ocs { code: _, .. } => OcsStatus::UnknownError, // raw code already in message
            CoreError::Config(_)
            | CoreError::ConfigValidation(_)
            | CoreError::Db(_)
            | CoreError::Cache(_)
            | CoreError::Internal(_) => OcsStatus::ServerError,
        }
    }
}

impl IntoResponse for OcsError {
    fn into_response(self) -> Response {
        let status = self.ocs_status();
        let http_status = StatusCode::from_u16(self.error.http_status())
            .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let envelope = OcsResponse {
            status,
            message: self.error.client_message(),
            data: serde_json::Value::Null,
            version: self.version,
        };
        let (body, ct) = render(&envelope, self.format);
        (http_status, [(header::CONTENT_TYPE, ct)], body).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    #[tokio::test]
    async fn api_error_emits_json_error_body() {
        let resp = ApiError(CoreError::NotFound).into_response();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let body = to_bytes(resp.into_body(), 1024).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["error"], "Not Found");
    }

    #[tokio::test]
    async fn api_error_masks_internal_details() {
        let inner = CoreError::Internal(anyhow::anyhow!("connection pool exhausted: 42 waiting"));
        let resp = ApiError(inner).into_response();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = to_bytes(resp.into_body(), 1024).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["error"], "Internal Server Error"); // generic; no leak
    }

    #[tokio::test]
    async fn ocs_error_emits_xml_envelope_by_default() {
        let err = OcsError::new(CoreError::NotFound, OcsVersion::V2, Format::Xml);
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
        let ct = resp.headers().get(header::CONTENT_TYPE).unwrap();
        assert!(ct.to_str().unwrap().starts_with("application/xml"));
        let body = to_bytes(resp.into_body(), 1024).await.unwrap();
        let body_s = String::from_utf8(body.to_vec()).unwrap();
        assert!(body_s.contains("<statuscode>998</statuscode>"));
        assert!(body_s.contains("<message>Not Found</message>"));
    }

    #[tokio::test]
    async fn ocs_error_emits_json_when_format_json() {
        let err = OcsError::new(CoreError::Unauthorized, OcsVersion::V2, Format::Json);
        let resp = err.into_response();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let ct = resp.headers().get(header::CONTENT_TYPE).unwrap();
        assert!(ct.to_str().unwrap().starts_with("application/json"));
        let body = to_bytes(resp.into_body(), 1024).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["ocs"]["meta"]["statuscode"], 997);
        assert_eq!(parsed["ocs"]["meta"]["status"], "failure");
    }
}
