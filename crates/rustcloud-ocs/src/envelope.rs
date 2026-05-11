//! OCS envelope rendering. JSON via `serde_json`; XML is hand-rolled — Nextcloud's
//! envelope wants a specific JSON-as-XML shape that's painful to produce via
//! `quick-xml`'s typed serializer, so we walk the JSON tree and emit elements
//! directly.

use crate::format::Format;
use crate::status::{OcsStatus, OcsVersion};
use serde::Serialize;
use serde_json::{json, Value};

#[derive(Debug)]
pub struct OcsResponse<T: Serialize> {
    pub status: OcsStatus,
    pub message: String,
    pub data: T,
    pub version: OcsVersion,
}

impl<T: Serialize> OcsResponse<T> {
    pub fn ok(data: T, version: OcsVersion) -> Self {
        Self {
            status: OcsStatus::Ok,
            message: "OK".into(),
            data,
            version,
        }
    }

    pub fn failure(
        status: OcsStatus,
        message: impl Into<String>,
        data: T,
        version: OcsVersion,
    ) -> Self {
        Self {
            status,
            message: message.into(),
            data,
            version,
        }
    }
}

/// Render to `(body, content_type)`. Errors are infallible at this layer
/// because serde_json::to_string only fails on user-supplied types we don't
/// pass through; we wrap the JSON case in a panic-free pattern anyway.
pub fn render<T: Serialize>(resp: &OcsResponse<T>, format: Format) -> (String, &'static str) {
    match format {
        Format::Json => (render_json(resp), Format::Json.content_type()),
        Format::Xml => (render_xml(resp), Format::Xml.content_type()),
    }
}

fn render_json<T: Serialize>(resp: &OcsResponse<T>) -> String {
    let meta = json!({
        "status": resp.status.label(),
        "statuscode": resp.status.code_for(resp.version),
        "message": resp.message,
    });
    let data: Value = serde_json::to_value(&resp.data).unwrap_or(Value::Null);
    let envelope = json!({
        "ocs": {
            "meta": meta,
            "data": data,
        }
    });
    serde_json::to_string(&envelope).unwrap_or_else(|_| "{}".into())
}

fn render_xml<T: Serialize>(resp: &OcsResponse<T>) -> String {
    // Build a JSON value first, then walk it as a tree to emit XML by hand.
    // Nextcloud's XML format is "JSON-as-XML": numbers, strings, arrays, objects
    // all map mechanically.
    let data: Value = serde_json::to_value(&resp.data).unwrap_or(Value::Null);
    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\"?>\n<ocs>\n");
    out.push_str("  <meta>\n");
    out.push_str(&format!(
        "    <status>{}</status>\n",
        xml_escape(resp.status.label())
    ));
    out.push_str(&format!(
        "    <statuscode>{}</statuscode>\n",
        resp.status.code_for(resp.version)
    ));
    out.push_str(&format!(
        "    <message>{}</message>\n",
        xml_escape(&resp.message)
    ));
    out.push_str("  </meta>\n");
    out.push_str("  <data>");
    write_value(&mut out, &data);
    out.push_str("</data>\n");
    out.push_str("</ocs>\n");
    out
}

fn write_value(out: &mut String, v: &Value) {
    match v {
        Value::Null => {}
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => out.push_str(&xml_escape(s)),
        Value::Array(arr) => {
            for item in arr {
                out.push_str("<element>");
                write_value(out, item);
                out.push_str("</element>");
            }
        }
        Value::Object(map) => {
            for (k, v) in map {
                let tag = xml_escape(k);
                out.push_str(&format!("<{tag}>"));
                write_value(out, v);
                out.push_str(&format!("</{tag}>"));
            }
        }
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Serialize;

    #[derive(Serialize)]
    struct Empty {}

    #[derive(Serialize)]
    struct VersionPayload {
        major: u32,
        minor: u32,
        edition: String,
    }

    #[test]
    fn ok_json_envelope_v2() {
        let r = OcsResponse::ok(Empty {}, OcsVersion::V2);
        let (body, ct) = render(&r, Format::Json);
        assert!(ct.starts_with("application/json"));
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["ocs"]["meta"]["status"], "ok");
        assert_eq!(parsed["ocs"]["meta"]["statuscode"], 200);
        assert_eq!(parsed["ocs"]["meta"]["message"], "OK");
    }

    #[test]
    fn ok_xml_envelope_v1() {
        let r = OcsResponse::ok(Empty {}, OcsVersion::V1);
        let (body, _) = render(&r, Format::Xml);
        assert!(body.contains("<status>ok</status>"));
        assert!(body.contains("<statuscode>100</statuscode>"));
        assert!(body.contains("<message>OK</message>"));
    }

    #[test]
    fn failure_carries_message_and_code() {
        let r = OcsResponse::failure(
            OcsStatus::NotFound,
            "no such user",
            Empty {},
            OcsVersion::V2,
        );
        let (body_json, _) = render(&r, Format::Json);
        let parsed: serde_json::Value = serde_json::from_str(&body_json).unwrap();
        assert_eq!(parsed["ocs"]["meta"]["status"], "failure");
        assert_eq!(parsed["ocs"]["meta"]["statuscode"], 998);
        assert_eq!(parsed["ocs"]["meta"]["message"], "no such user");
    }

    #[test]
    fn json_payload_round_trip() {
        let payload = VersionPayload {
            major: 31,
            minor: 0,
            edition: "Rustcloud".into(),
        };
        let r = OcsResponse::ok(payload, OcsVersion::V2);
        let (body, _) = render(&r, Format::Json);
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["ocs"]["data"]["major"], 31);
        assert_eq!(parsed["ocs"]["data"]["minor"], 0);
        assert_eq!(parsed["ocs"]["data"]["edition"], "Rustcloud");
    }

    #[test]
    fn xml_payload_emits_nested_tags() {
        let payload = VersionPayload {
            major: 31,
            minor: 0,
            edition: "Rustcloud".into(),
        };
        let r = OcsResponse::ok(payload, OcsVersion::V2);
        let (body, _) = render(&r, Format::Xml);
        assert!(body.contains("<major>31</major>"));
        assert!(body.contains("<minor>0</minor>"));
        assert!(body.contains("<edition>Rustcloud</edition>"));
    }

    #[test]
    fn xml_escapes_special_chars() {
        let r = OcsResponse::failure(
            OcsStatus::BadRequest,
            "5 < 6 & true",
            Empty {},
            OcsVersion::V2,
        );
        let (body, _) = render(&r, Format::Xml);
        assert!(body.contains("<message>5 &lt; 6 &amp; true</message>"));
    }
}
