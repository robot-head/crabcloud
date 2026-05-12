//! Shared XML helpers for DAV responses. Uses `quick_xml::writer::Writer`.
//!
//! All helpers emit prefixed names (`d:`, `oc:`, `nc:`). The outer
//! `multistatus` writer declares the matching `xmlns:*` attributes so
//! children can reference any of the three namespaces without redeclaring.

use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};
use quick_xml::Writer;
use std::io::Cursor;

/// Build a `<d:multistatus>` document. The supplied closure emits one or
/// more `<d:response>` blocks via [`write_response`]. The XML declaration
/// and namespace bindings are written automatically.
pub fn multistatus<F>(build_responses: F) -> Vec<u8>
where
    F: FnOnce(&mut Writer<Cursor<Vec<u8>>>) -> Result<(), quick_xml::Error>,
{
    let mut w = Writer::new(Cursor::new(Vec::new()));
    let _ = w.write_event(Event::Decl(quick_xml::events::BytesDecl::new(
        "1.0",
        Some("utf-8"),
        None,
    )));
    let mut start = BytesStart::new("d:multistatus");
    start.push_attribute(("xmlns:d", "DAV:"));
    start.push_attribute(("xmlns:oc", "http://owncloud.org/ns"));
    start.push_attribute(("xmlns:nc", "http://nextcloud.org/ns"));
    let _ = w.write_event(Event::Start(start));
    let _ = build_responses(&mut w);
    let _ = w.write_event(Event::End(BytesEnd::new("d:multistatus")));
    w.into_inner().into_inner()
}

/// Write a single `<d:response>` with the supplied href + one-or-more
/// propstat blocks emitted by the closure.
pub fn write_response<F>(
    w: &mut Writer<Cursor<Vec<u8>>>,
    href: &str,
    build_propstats: F,
) -> Result<(), quick_xml::Error>
where
    F: FnOnce(&mut Writer<Cursor<Vec<u8>>>) -> Result<(), quick_xml::Error>,
{
    w.write_event(Event::Start(BytesStart::new("d:response")))?;
    w.write_event(Event::Start(BytesStart::new("d:href")))?;
    w.write_event(Event::Text(BytesText::new(href)))?;
    w.write_event(Event::End(BytesEnd::new("d:href")))?;
    build_propstats(w)?;
    w.write_event(Event::End(BytesEnd::new("d:response")))?;
    Ok(())
}

/// Write a `<d:propstat>` containing a `<d:prop>` block populated by the
/// closure plus the supplied `<d:status>` line.
pub fn write_propstat<F>(
    w: &mut Writer<Cursor<Vec<u8>>>,
    status: &str,
    build_props: F,
) -> Result<(), quick_xml::Error>
where
    F: FnOnce(&mut Writer<Cursor<Vec<u8>>>) -> Result<(), quick_xml::Error>,
{
    w.write_event(Event::Start(BytesStart::new("d:propstat")))?;
    w.write_event(Event::Start(BytesStart::new("d:prop")))?;
    build_props(w)?;
    w.write_event(Event::End(BytesEnd::new("d:prop")))?;
    w.write_event(Event::Start(BytesStart::new("d:status")))?;
    w.write_event(Event::Text(BytesText::new(status)))?;
    w.write_event(Event::End(BytesEnd::new("d:status")))?;
    w.write_event(Event::End(BytesEnd::new("d:propstat")))?;
    Ok(())
}

/// Helper: write a leaf element with text content.
pub fn write_leaf(
    w: &mut Writer<Cursor<Vec<u8>>>,
    name: &str,
    text: &str,
) -> Result<(), quick_xml::Error> {
    w.write_event(Event::Start(BytesStart::new(name)))?;
    w.write_event(Event::Text(BytesText::new(text)))?;
    w.write_event(Event::End(BytesEnd::new(name)))?;
    Ok(())
}

/// Helper: write an empty self-closing element.
pub fn write_empty(w: &mut Writer<Cursor<Vec<u8>>>, name: &str) -> Result<(), quick_xml::Error> {
    w.write_event(Event::Empty(BytesStart::new(name)))?;
    Ok(())
}
