//! `OcsFormat` extractor — resolves the response format from `?format=`
//! query string or the `Accept` request header, mirroring Nextcloud's
//! content-negotiation rules.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use rustcloud_ocs::{negotiate, Format};
use std::convert::Infallible;

/// Axum extractor that resolves the desired OCS response [`Format`] from the
/// request's `?format=` query parameter and `Accept` header.
#[derive(Debug, Clone, Copy)]
pub struct OcsFormat(pub Format);

impl<S> FromRequestParts<S> for OcsFormat
where
    S: Send + Sync,
{
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let format_query: Option<String> = parts.uri.query().and_then(|q| {
            q.split('&').find_map(|kv| {
                kv.split_once('=')
                    .and_then(|(k, v)| (k == "format").then(|| v.to_string()))
            })
        });
        let accept = parts
            .headers
            .get(axum::http::header::ACCEPT)
            .and_then(|v| v.to_str().ok())
            .map(String::from);
        Ok(OcsFormat(negotiate(
            format_query.as_deref(),
            accept.as_deref(),
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::get;
    use axum::Router;
    use tower::ServiceExt;

    async fn echo_format(f: OcsFormat) -> &'static str {
        match f.0 {
            Format::Json => "json",
            Format::Xml => "xml",
        }
    }

    fn app() -> Router {
        Router::new().route("/x", get(echo_format))
    }

    #[tokio::test]
    async fn defaults_to_xml() {
        let req = Request::builder().uri("/x").body(Body::empty()).unwrap();
        let resp = app().oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 16).await.unwrap();
        assert_eq!(&body[..], b"xml");
    }

    #[tokio::test]
    async fn query_param_selects_json() {
        let req = Request::builder()
            .uri("/x?format=json")
            .body(Body::empty())
            .unwrap();
        let resp = app().oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 16).await.unwrap();
        assert_eq!(&body[..], b"json");
    }

    #[tokio::test]
    async fn accept_header_selects_json() {
        let req = Request::builder()
            .uri("/x")
            .header("accept", "application/json")
            .body(Body::empty())
            .unwrap();
        let resp = app().oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(resp.into_body(), 16).await.unwrap();
        assert_eq!(&body[..], b"json");
    }
}
