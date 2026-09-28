//! Paragraph styles from `word/styles.xml`: names, outline levels and
//! style-level numbering, resolved through the `basedOn` chain.

use std::collections::HashMap;

use quick_xml::Reader;
use quick_xml::events::Event;

use crate::error::{Error, Result};
use crate::model::NumberingRef;
use crate::xml::{attr, local, skip};

const PART: &str = "word/styles.xml";

#[derive(Debug, Default, Clone)]
struct Style {
    name: Option<String>,
    based_on: Option<String>,
    outline_level: Option<u8>,
    num_id: Option<u32>,
    num_level: Option<u8>,
}

#[derive(Debug, Default)]
pub struct Styles {
    styles: HashMap<String, Style>,
    default_paragraph: Option<String>,
}

impl Styles {
    pub fn parse(xml: &str) -> Result<Self> {
        let mut reader = Reader::from_str(xml);
        let mut styles = Styles::default();
        let mut current: Option<(String, Style)> = None;
        let mut in_ppr = false;
        let mut in_numpr = false;
        loop {
            let event = reader.read_event().map_err(|e| Error::xml(PART, e))?;
            match &event {
                Event::Start(e) | Event::Empty(e) => {
                    let name = local(e);
                    match name.as_str() {
                        "style" => {
                            let is_empty = matches!(event, Event::Empty(_));
                            if attr(e, "type").as_deref() != Some("paragraph") {
                                if !is_empty {
                                    skip(&mut reader, e, PART)?;
                                }
                                continue;
                            }
                            let id = attr(e, "styleId").unwrap_or_default();
                            if attr(e, "default")
                                .as_deref()
                                .is_some_and(|v| v == "1" || v == "true")
                            {
                                styles.default_paragraph = Some(id.clone());
                            }
                            if is_empty {
                                styles.styles.insert(id, Style::default());
                            } else {
                                current = Some((id, Style::default()));
                            }
                        }
                        "rPr" | "tblPr" | "trPr" | "tcPr" | "tblStylePr" if current.is_some() => {
                            if matches!(event, Event::Start(_)) {
                                skip(&mut reader, e, PART)?;
                            }
                        }
                        _ => {
                            if let Some((_, style)) = current.as_mut() {
                                match name.as_str() {
                                    "name" => style.name = attr(e, "val"),
                                    "basedOn" => style.based_on = attr(e, "val"),
                                    "pPr" => in_ppr = matches!(event, Event::Start(_)),
                                    "outlineLvl" if in_ppr => {
                                        style.outline_level =
                                            attr(e, "val").and_then(|v| v.parse().ok())
                                    }
                                    "numPr" if in_ppr => {
                                        in_numpr = matches!(event, Event::Start(_))
                                    }
                                    "numId" if in_numpr => {
                                        style.num_id = attr(e, "val").and_then(|v| v.parse().ok())
                                    }
                                    "ilvl" if in_numpr => {
                                        style.num_level =
                                            attr(e, "val").and_then(|v| v.parse().ok())
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                }
                Event::End(e) => match e.local_name().as_ref() {
                    "style" => {
                        if let Some((id, style)) = current.take() {
                            styles.styles.insert(id, style);
                        }
                    }
                    "pPr" => in_ppr = false,
                    "numPr" => in_numpr = false,
                    _ => {}
                },
                Event::Eof => break,
                _ => {}
            }
        }
        Ok(styles)
    }

    fn chain(&self, style_id: Option<&str>) -> impl Iterator<Item = &Style> {
        let mut next = style_id
            .map(str::to_string)
            .or_else(|| self.default_paragraph.clone());
        let mut steps = 0;
        std::iter::from_fn(move || {
            if steps > 16 {
                return None;
            }
            steps += 1;
            let style = self.styles.get(next.as_ref()?)?;
            next = style.based_on.clone();
            Some(style)
        })
    }

    pub fn name(&self, style_id: &str) -> Option<&str> {
        self.styles.get(style_id)?.name.as_deref()
    }

    /// Heading level (0 = heading 1) implied by the style, if any.
    pub fn outline_level(&self, style_id: Option<&str>) -> Option<u8> {
        for style in self.chain(style_id) {
            if let Some(level) = style.outline_level {
                return (level < 9).then_some(level);
            }
            if let Some(level) = style.name.as_deref().and_then(heading_level_from_name) {
                return Some(level);
            }
        }
        None
    }

    pub fn numbering(&self, style_id: Option<&str>) -> Option<NumberingRef> {
        let mut num_id = None;
        let mut level = None;
        for style in self.chain(style_id) {
            num_id = num_id.or(style.num_id);
            level = level.or(style.num_level);
        }
        num_id.map(|num_id| NumberingRef {
            num_id,
            level: level.unwrap_or(0),
        })
    }
}

fn heading_level_from_name(name: &str) -> Option<u8> {
    let lower = name.to_lowercase();
    let rest = lower
        .strip_prefix("heading ")
        .or_else(|| lower.strip_prefix("标题 "))
        .or_else(|| lower.strip_prefix("标题"))?;
    let n: u8 = rest.trim().parse().ok()?;
    (1..=9).contains(&n).then(|| n - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_heading_levels_through_based_on() {
        let xml = r#"<w:styles xmlns:w="w">
            <w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style>
            <w:style w:type="paragraph" w:styleId="1"><w:name w:val="heading 1"/><w:basedOn w:val="Normal"/>
              <w:pPr><w:outlineLvl w:val="0"/></w:pPr><w:rPr><w:b/></w:rPr></w:style>
            <w:style w:type="paragraph" w:styleId="Custom"><w:name w:val="我的标题"/><w:basedOn w:val="1"/></w:style>
            <w:style w:type="paragraph" w:styleId="H2"><w:name w:val="标题 2"/></w:style>
            <w:style w:type="character" w:styleId="Emph"><w:name w:val="heading 3"/></w:style>
        </w:styles>"#;
        let styles = Styles::parse(xml).unwrap();
        assert_eq!(styles.outline_level(Some("1")), Some(0));
        assert_eq!(styles.outline_level(Some("Custom")), Some(0));
        assert_eq!(styles.outline_level(Some("H2")), Some(1));
        assert_eq!(styles.outline_level(Some("Emph")), None);
        assert_eq!(styles.outline_level(None), None);
    }
}
