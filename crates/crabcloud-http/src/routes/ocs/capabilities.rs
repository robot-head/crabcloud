//! `GET /ocs/v2.php/cloud/capabilities`.

use crate::extractors::auth::OptionalUser;
use crate::extractors::format::OcsFormat;
use crate::OcsError;
use axum::extract::State;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use crabcloud_core::{AppState, Error as CoreError};
use crabcloud_ocs::{aggregate, render, CapabilityContext, OcsResponse, OcsStatus, OcsVersion};

pub async fn handler(
    State(state): State<AppState>,
    OptionalUser(user): OptionalUser,
    fmt: OcsFormat,
) -> Result<Response, OcsError> {
    let user_id = user.as_ref().map(|u| u.user_id.clone());
    let ctx = CapabilityContext {
        locale: None,
        user_id: user_id.as_deref(),
    };
    let providers = state.capability_providers.lock().await.clone();
    let payload = aggregate(
        &providers,
        &ctx,
        state.cache.clone(),
        &state.config.versionstring,
        &state.config.instanceid,
    )
    .await
    .map_err(|e| {
        OcsError::new(
            CoreError::Internal(anyhow::anyhow!("caps: {e}")),
            OcsVersion::V2,
            fmt.0,
        )
    })?;

    let envelope = OcsResponse {
        status: OcsStatus::Ok,
        message: "OK".into(),
        data: payload.body,
        version: OcsVersion::V2,
    };
    let (body, ct) = render(&envelope, fmt.0);
    let mut headers = HeaderMap::new();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(ct));
    if let Ok(etag) = HeaderValue::from_str(&payload.etag) {
        headers.insert(header::ETAG, etag);
    }
    Ok((StatusCode::OK, headers, body).into_response())
}

#[cfg(test)]
mod tests {
    use crate::router::build_router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use crabcloud_config::test_support::minimal_sqlite_config;
    use crabcloud_core::AppStateBuilder;
    use tempfile::tempdir;
    use tower::ServiceExt;

    #[tokio::test]
    async fn capabilities_xml_default() {
        let dir = tempdir().unwrap();
        let cfg = minimal_sqlite_config(dir.path().join("caps.db"));
        let state = AppStateBuilder::new(cfg)
            .with_core_capabilities()
            .build()
            .await
            .unwrap();
        let app = build_router(state, axum::Router::new());

        let req = Request::builder()
            .uri("/ocs/v2.php/cloud/capabilities")
            .header("ocs-apirequest", "true")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(ct.starts_with("application/xml"));
        let body = axum::body::to_bytes(resp.into_body(), 16 * 1024)
            .await
            .unwrap();
        let s = String::from_utf8_lossy(&body);
        assert!(s.contains("<statuscode>200</statuscode>"));
        assert!(s.contains("<pollinterval>60</pollinterval>"));
    }

    #[tokio::test]
    async fn capabilities_json_via_query() {
        let dir = tempdir().unwrap();
        let cfg = minimal_sqlite_config(dir.path().join("caps.db"));
        let state = AppStateBuilder::new(cfg)
            .with_core_capabilities()
            .build()
            .await
            .unwrap();
        let app = build_router(state, axum::Router::new());

        let req = Request::builder()
            .uri("/ocs/v2.php/cloud/capabilities?format=json")
            .header("ocs-apirequest", "true")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), 16 * 1024)
            .await
            .unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["ocs"]["meta"]["statuscode"], 200);
        assert_eq!(
            parsed["ocs"]["data"]["capabilities"]["core"]["pollinterval"],
            60
        );
    }
}
