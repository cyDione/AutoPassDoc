//! Small helpers over quick-xml for WordprocessingML, where element and
//! attribute prefixes vary between producers (w:, w14:, w15: ...), so
//! lookups go by local name.

use quick_xml::events::{BytesRef, BytesStart, Event};
use quick_xml::{Reader, XmlVersion};

use crate::error::{Error, Result};

pub fn local(e: &BytesStart<'_>) -> String {
    e.local_name().as_ref().to_string()
}

/// Value of the first attribute whose local name matches, unescaped.
pub fn attr(e: &BytesStart<'_>, name: &str) -> Option<String> {
    e.attributes().with_checks(false).flatten().find_map(|a| {
        (a.key.local_name().as_ref() == name)
            .then(|| {
                a.normalized_value(XmlVersion::Implicit1_0)
                    .ok()
                    .map(|v| v.into_owned())
            })
            .flatten()
    })
}

/// Interprets an OOXML on/off property such as `<w:b/>` or `<w:b w:val="0"/>`.
pub fn toggle(e: &BytesStart<'_>) -> bool {
    !matches!(
        attr(e, "val").as_deref(),
        Some("0") | Some("false") | Some("off") | Some("none")
    )
}

pub fn resolve_ref(r: &BytesRef<'_>) -> Option<char> {
    if r.is_char_ref() {
        return r.resolve_char_ref().ok().flatten();
    }
    match &**r {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        _ => None,
    }
}

/// Skips the rest of the element that `start` opened.
pub fn skip(reader: &mut Reader<&[u8]>, start: &BytesStart<'_>, part: &str) -> Result<()> {
    reader
        .read_to_end(start.name())
        .map(|_| ())
        .map_err(|e| Error::xml(part, e))
}

/// Collects the character content up to the end of the element that `start` opened.
pub fn read_text(reader: &mut Reader<&[u8]>, start: &BytesStart<'_>, part: &str) -> Result<String> {
    let mut out = String::new();
    let end = start.name().as_ref().to_string();
    let mut depth = 0usize;
    loop {
        match reader.read_event().map_err(|e| Error::xml(part, e))? {
            Event::Text(t) => out.push_str(&t.xml10_content()),
            Event::CData(t) => out.push_str(&t.xml10_content()),
            Event::GeneralRef(r) => out.extend(resolve_ref(&r)),
            Event::Start(e) if e.name().as_ref() == end => depth += 1,
            Event::End(e) if e.name().as_ref() == end => {
                if depth == 0 {
                    return Ok(out);
                }
                depth -= 1;
            }
            Event::Eof => return Err(Error::xml(part, "unexpected end of file")),
            _ => {}
        }
    }
}

pub fn to_str<'a>(bytes: &'a [u8], part: &str) -> Result<&'a str> {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    std::str::from_utf8(bytes).map_err(|e| Error::xml(part, e))
}
