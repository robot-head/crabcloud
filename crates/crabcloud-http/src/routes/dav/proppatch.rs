//! PROPPATCH handler. Parses `<d:set>` / `<d:remove>` operations from the
//! request body and applies them via [`PropertyStore`]. Protected props
//! (spec §8.2 — live props the server owns) are rejected with `403` in
//! the per-prop `<d:propstat>` block; the overall response is always
//! `207 Multi-Status` so the client sees per-prop status.

use crabcloud_core::AppState;
use crabcloud_filecache::PropertyStore;
use crabcloud_fs::UserPath;
use crabcloud_users::UserId;
use quick_xml::events::Event;
use quick_xml::reader::Reader;

use crate::routes::dav::error::{DavError, DavResult};
use crate::routes::dav::xml::{multistatus, write_leaf, write_propstat, write_response};
use axum::body::Body;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};

/// Props the server computes (spec §8.2). Attempts to set/remove these are
/// rejected with `403 Forbidden` in their own propstat block. Names are in
/// Clark notation (`{namespace}localname`) — matches `parse_body`'s output.
const PROTECTED_PROPS: &[&str] = &[
    "{DAV:}getetag",
    "{DAV:}getcontentlength",
    "{DAV:}getlastmodified",
    "{DAV:}getcontenttype",
    "{DAV:}resourcetype",
    "{DAV:}displayname",
    "{http://owncloud.org/ns}id",
    "{http://owncloud.org/ns}permissions",
    "{http://owncloud.org/ns}size",
];

#[derive(Debug, PartialEq, Eq)]
enum PropOp {
    Set { name: String, value: Option<String> },
    Remove { name: String },
}

/// Parse the PROPPATCH request body. Walks the XML stream tracking the
/// in-scope `xmlns:*` declarations so prefixed element names can be lifted
/// to Clark notation (`{namespace}local`). Structural element matching
/// (`set` / `remove` / `prop` / `propertyupdate`) is performed against the
/// resolved Clark name so that clients using arbitrary prefix names (e.g.
/// `<a:set xmlns:a="DAV:">`) are handled correctly. The parser is
/// intentionally lax about ordering and whitespace.
fn parse_body(body: &[u8]) -> DavResult<Vec<PropOp>> {
    let mut reader = Reader::from_reader(body);
    reader.config_mut().trim_text(true);
    let mut ops = Vec::new();
    let mut mode: Option<&'static str> = None; // "set" or "remove"
    let mut current_name: Option<String> = None;
    let mut current_value: Option<String> = None;
    let mut current_ns_prefix: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let name_bytes = e.name();
                let raw = std::str::from_utf8(name_bytes.as_ref())
                    .map_err(|_| DavError::BadRequest("non-utf8 prop name".into()))?
                    .to_string();
                capture_xmlns_attrs(&e, &mut current_ns_prefix)?;
                let clark = resolved_clark(&raw, &current_ns_prefix);
                match clark.as_str() {
                    "{DAV:}set" => mode = Some("set"),
                    "{DAV:}remove" => mode = Some("remove"),
                    "{DAV:}prop" | "{DAV:}propertyupdate" => {}
                    _ => {
                        if mode.is_some() && current_name.is_none() {
                            current_name = Some(clark);
                            current_value = Some(String::new());
                        }
                    }
                }
            }
            Ok(Event::Empty(e)) => {
                // Self-closing element. For prop names we still want to
                // record an op; for `<d:set/>` / `<d:remove/>` there is no
                // op (no inner prop), so ignore.
                let name_bytes = e.name();
                let raw = std::str::from_utf8(name_bytes.as_ref())
                    .map_err(|_| DavError::BadRequest("non-utf8 prop name".into()))?
                    .to_string();
                capture_xmlns_attrs(&e, &mut current_ns_prefix)?;
                let clark = resolved_clark(&raw, &current_ns_prefix);
                match clark.as_str() {
                    "{DAV:}set" | "{DAV:}remove" | "{DAV:}prop" | "{DAV:}propertyupdate" => {}
                    _ => {
                        if let Some(m) = mode {
                            match m {
                                "set" => ops.push(PropOp::Set {
                                    name: clark,
                                    value: None,
                                }),
                                "remove" => ops.push(PropOp::Remove { name: clark }),
                                _ => {}
                            }
                        }
                    }
                }
            }
            Ok(Event::Text(t)) => {
                if let Some(v) = current_value.as_mut() {
                    if let Ok(decoded) = t.decode() {
                        v.push_str(decoded.as_ref());
                    }
                }
            }
            Ok(Event::End(e)) => {
                let name_bytes = e.name();
                let raw = std::str::from_utf8(name_bytes.as_ref())
                    .map_err(|_| DavError::BadRequest("non-utf8 prop name".into()))?
                    .to_string();
                let clark = resolved_clark(&raw, &current_ns_prefix);
                match clark.as_str() {
                    "{DAV:}set" | "{DAV:}remove" => mode = None,
                    "{DAV:}prop" | "{DAV:}propertyupdate" => {}
                    _ => {
                        if let Some(name) = current_name.take() {
                            let value = current_value.take();
                            let value = value.filter(|s| !s.is_empty());
                            match mode {
                                Some("set") => ops.push(PropOp::Set { name, value }),
                                Some("remove") => ops.push(PropOp::Remove { name }),
                                _ => {}
                            }
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(DavError::BadRequest(format!("xml parse: {e}"))),
            _ => {}
        }
    }
    Ok(ops)
}

/// Resolve a (possibly prefixed) element name to Clark notation
/// (`{namespace}local`) using the in-scope prefix map. If the prefix is
/// unbound the local part is returned bare — see `name_to_clark` for the
/// rationale behind this lenient fallback. Used at both `Event::Start` and
/// `Event::End` sites so structural-element matching is consistent.
fn resolved_clark(name: &str, prefixes: &std::collections::HashMap<String, String>) -> String {
    name_to_clark(name, prefixes)
}

fn capture_xmlns_attrs(
    e: &quick_xml::events::BytesStart,
    map: &mut std::collections::HashMap<String, String>,
) -> DavResult<()> {
    for attr in e.attributes().flatten() {
        let k = std::str::from_utf8(attr.key.as_ref())
            .map_err(|_| DavError::BadRequest("non-utf8 attr".into()))?
            .to_string();
        let v = std::str::from_utf8(&attr.value)
            .map_err(|_| DavError::BadRequest("non-utf8 attr value".into()))?
            .to_string();
        if let Some(prefix) = k.strip_prefix("xmlns:") {
            map.insert(prefix.to_string(), v);
        } else if k == "xmlns" {
            map.insert(String::new(), v);
        }
    }
    Ok(())
}

/// Convert a prefixed element name (`oc:favorite`) to Clark notation
/// (`{http://owncloud.org/ns}favorite`) using the in-scope prefix map.
/// Falls back to the element's local name if the prefix is unbound — this
/// is lenient by design, mirroring the indulgence Nextcloud's own parser
/// shows to slightly malformed client payloads.
fn name_to_clark(name: &str, prefixes: &std::collections::HashMap<String, String>) -> String {
    if let Some((prefix, local)) = name.split_once(':') {
        if let Some(ns) = prefixes.get(prefix) {
            return format!("{{{}}}{}", ns, local);
        }
        return local.to_string();
    }
    if let Some(ns) = prefixes.get("") {
        return format!("{{{}}}{}", ns, name);
    }
    name.to_string()
}

/// Handle a `PROPPATCH` request (RFC 4918 §9.2). Parses the XML body for
/// `<d:set>` / `<d:remove>` operations, applies non-protected props via
/// [`PropertyStore`], and returns a `207 Multi-Status` response with a
/// per-prop status block. Protected (server-computed) props produce
/// `403 Forbidden` in their own propstat; the overall HTTP status is
/// always `207` so the client can see each prop's outcome individually.
pub async fn handle(
    state: AppState,
    uid: &UserId,
    user_path: &UserPath,
    headers: &HeaderMap,
    body: Body,
) -> DavResult<Response> {
    // Lock-aware: PROPPATCH mutates per-resource state and is gated by
    // the same If-token contract as PUT/MKCOL/DELETE/MOVE/COPY.
    let locks = crabcloud_filecache::LockStore::new(state.filecache.pool().clone());
    crate::routes::dav::lock::lock_check(&locks, uid, user_path, headers).await?;
    // Verify resource exists (404 → propagated up).
    let view = state.view_for(uid).await?;
    let _meta = view.stat(user_path).await?;

    let body_bytes = axum::body::to_bytes(body, 1024 * 1024)
        .await
        .map_err(|e| DavError::BadRequest(format!("body read: {e}")))?;
    let ops = parse_body(&body_bytes)?;
    let store = PropertyStore::new(state.filecache.pool().clone());
    let property_path = user_path.as_str().trim_start_matches('/').to_string();

    let mut results: Vec<(String, &'static str)> = Vec::new();
    for op in ops {
        match op {
            PropOp::Set { name, value } => {
                if PROTECTED_PROPS.contains(&name.as_str()) {
                    results.push((name, "HTTP/1.1 403 Forbidden"));
                    continue;
                }
                store
                    .upsert(uid, &property_path, &name, value.as_deref())
                    .await?;
                results.push((name, "HTTP/1.1 200 OK"));
            }
            PropOp::Remove { name } => {
                if PROTECTED_PROPS.contains(&name.as_str()) {
                    results.push((name, "HTTP/1.1 403 Forbidden"));
                    continue;
                }
                store.delete(uid, &property_path, &name).await?;
                results.push((name, "HTTP/1.1 200 OK"));
            }
        }
    }

    // PROPPATCH echo: emit each prop with its outcome's status. We render
    // the Clark-notation name as a `clark:<name>` literal — the client only
    // pattern-matches by status anyway, but rendering the actual property
    // tag (with proper prefix) would require a reverse Clark→prefix table.
    // SP5 sidesteps that here by emitting an `<oc:*>` / `<d:*>` form by
    // walking the same prefix conventions; unknown namespaces fall through
    // to a raw `<x:_>` style placeholder. In practice clients only care
    // about the propstat's status code.
    let prefix = "/remote.php/dav/files";
    let href = format!("{}/{}{}", prefix, uid.as_str(), user_path.as_str());
    let body = multistatus(|w| {
        write_response(w, &href, |w| {
            for (name, status) in &results {
                let tag = clark_to_prefixed_tag(name);
                write_propstat(w, status, |w| write_leaf(w, &tag, ""))?;
            }
            Ok(())
        })
    });

    Ok((
        StatusCode::from_u16(207).expect("207 is a valid status code"),
        [(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/xml; charset=utf-8"),
        )],
        Body::from(body),
    )
        .into_response())
}

/// Render a Clark-notation name as a prefixed tag suitable for the response
/// body. The outer `<d:multistatus>` declares `d:`, `oc:`, and `nc:` so
/// names in those namespaces become `d:foo` / `oc:foo` / `nc:foo`. Anything
/// outside that set falls back to the local name with no prefix.
fn clark_to_prefixed_tag(name: &str) -> String {
    if let Some(rest) = name.strip_prefix('{') {
        if let Some(close) = rest.find('}') {
            let ns = &rest[..close];
            let local = &rest[close + 1..];
            let prefix = match ns {
                "DAV:" => Some("d"),
                "http://owncloud.org/ns" => Some("oc"),
                "http://nextcloud.org/ns" => Some("nc"),
                _ => None,
            };
            return match prefix {
                Some(p) => format!("{}:{}", p, local),
                None => local.to_string(),
            };
        }
    }
    name.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_set_oc_favorite() {
        let body = br#"<?xml version="1.0"?>
<d:propertyupdate xmlns:d="DAV:" xmlns:oc="http://owncloud.org/ns">
  <d:set>
    <d:prop>
      <oc:favorite>1</oc:favorite>
    </d:prop>
  </d:set>
</d:propertyupdate>"#;
        let ops = parse_body(body).unwrap();
        assert_eq!(
            ops,
            vec![PropOp::Set {
                name: "{http://owncloud.org/ns}favorite".to_string(),
                value: Some("1".into()),
            }]
        );
    }

    #[test]
    fn parse_remove_oc_favorite_self_closing() {
        let body = br#"<?xml version="1.0"?>
<d:propertyupdate xmlns:d="DAV:" xmlns:oc="http://owncloud.org/ns">
  <d:remove>
    <d:prop>
      <oc:favorite/>
    </d:prop>
  </d:remove>
</d:propertyupdate>"#;
        let ops = parse_body(body).unwrap();
        assert_eq!(
            ops,
            vec![PropOp::Remove {
                name: "{http://owncloud.org/ns}favorite".to_string()
            }]
        );
    }

    #[test]
    fn parse_protected_prop_recognized() {
        let body = br#"<?xml version="1.0"?>
<d:propertyupdate xmlns:d="DAV:">
  <d:set><d:prop><d:getetag>"x"</d:getetag></d:prop></d:set>
</d:propertyupdate>"#;
        let ops = parse_body(body).unwrap();
        assert_eq!(ops.len(), 1);
        if let PropOp::Set { name, .. } = &ops[0] {
            assert_eq!(name, "{DAV:}getetag");
            assert!(PROTECTED_PROPS.contains(&name.as_str()));
        } else {
            panic!("expected Set");
        }
    }

    #[test]
    fn clark_to_prefixed_known_namespaces() {
        assert_eq!(
            clark_to_prefixed_tag("{http://owncloud.org/ns}favorite"),
            "oc:favorite"
        );
        assert_eq!(clark_to_prefixed_tag("{DAV:}displayname"), "d:displayname");
        assert_eq!(
            clark_to_prefixed_tag("{http://nextcloud.org/ns}note"),
            "nc:note"
        );
    }

    #[test]
    fn parse_with_uppercase_d_prefix_works() {
        let xml = r#"<?xml version="1.0"?>
<D:propertyupdate xmlns:D="DAV:" xmlns:oc="http://owncloud.org/ns">
  <D:set><D:prop><oc:favorite>1</oc:favorite></D:prop></D:set>
</D:propertyupdate>"#;
        let ops = parse_body(xml.as_bytes()).unwrap();
        assert_eq!(ops.len(), 1);
        match &ops[0] {
            PropOp::Set { name, value } => {
                assert_eq!(name, "{http://owncloud.org/ns}favorite");
                assert_eq!(value.as_deref(), Some("1"));
            }
            _ => panic!("expected Set"),
        }
    }

    #[test]
    fn parse_with_custom_prefix_works() {
        let xml = r#"<?xml version="1.0"?>
<a:propertyupdate xmlns:a="DAV:" xmlns:oc="http://owncloud.org/ns">
  <a:set><a:prop><oc:favorite>1</oc:favorite></a:prop></a:set>
</a:propertyupdate>"#;
        let ops = parse_body(xml.as_bytes()).unwrap();
        assert_eq!(ops.len(), 1);
        match &ops[0] {
            PropOp::Set { name, .. } => assert_eq!(name, "{http://owncloud.org/ns}favorite"),
            _ => panic!("expected Set"),
        }
    }

    #[test]
    fn parse_malformed_xml_returns_bad_request() {
        let xml = b"not <valid> XML <";
        let r = parse_body(xml);
        assert!(matches!(
            r,
            Err(crate::routes::dav::error::DavError::BadRequest(_))
        ));
    }
}
