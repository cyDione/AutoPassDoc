//! List numbering from `word/numbering.xml`, used to render labels such as
//! "一、", "（二）" or "1.2" that Word generates automatically.

use std::collections::HashMap;

use quick_xml::Reader;
use quick_xml::events::Event;

use crate::error::{Error, Result};
use crate::xml::{attr, local};

const PART: &str = "word/numbering.xml";

#[derive(Debug, Clone, Default)]
struct Level {
    start: u32,
    format: String,
    text: String,
}

#[derive(Debug, Default)]
pub struct Numbering {
    abstracts: HashMap<u32, Vec<Level>>,
    /// numId → (abstractNumId, per-level start overrides)
    nums: HashMap<u32, (u32, HashMap<u8, u32>)>,
}

impl Numbering {
    pub fn parse(xml: &str) -> Result<Self> {
        let mut reader = Reader::from_str(xml);
        let mut numbering = Numbering::default();
        let mut abstract_id: Option<u32> = None;
        let mut level: Option<(u8, Level)> = None;
        let mut num: Option<(u32, u32, HashMap<u8, u32>)> = None;
        let mut override_level: Option<u8> = None;
        loop {
            match reader.read_event().map_err(|e| Error::xml(PART, e))? {
                Event::Start(e) | Event::Empty(e) => {
                    let val = || attr(&e, "val");
                    match local(&e).as_str() {
                        "abstractNum" => {
                            abstract_id = attr(&e, "abstractNumId").and_then(|v| v.parse().ok())
                        }
                        "lvl" if abstract_id.is_some() => {
                            let ilvl = attr(&e, "ilvl").and_then(|v| v.parse().ok()).unwrap_or(0);
                            level = Some((
                                ilvl,
                                Level {
                                    start: 1,
                                    ..Level::default()
                                },
                            ));
                        }
                        "start" => {
                            if let Some((_, l)) = level.as_mut() {
                                l.start = val().and_then(|v| v.parse().ok()).unwrap_or(1);
                            }
                        }
                        "numFmt" => {
                            if let Some((_, l)) = level.as_mut() {
                                l.format = val().unwrap_or_default();
                            }
                        }
                        "lvlText" => {
                            if let Some((_, l)) = level.as_mut() {
                                l.text = val().unwrap_or_default();
                            }
                        }
                        "num" => {
                            let id = attr(&e, "numId").and_then(|v| v.parse().ok());
                            num = id.map(|id| (id, 0, HashMap::new()));
                        }
                        "abstractNumId" => {
                            if let Some(n) = num.as_mut() {
                                n.1 = val().and_then(|v| v.parse().ok()).unwrap_or(0);
                            }
                        }
                        "lvlOverride" => {
                            override_level = attr(&e, "ilvl").and_then(|v| v.parse().ok())
                        }
                        "startOverride" => {
                            if let (Some(n), Some(ilvl)) = (num.as_mut(), override_level)
                                && let Some(start) = val().and_then(|v| v.parse().ok())
                            {
                                n.2.insert(ilvl, start);
                            }
                        }
                        _ => {}
                    }
                }
                Event::End(e) => match e.local_name().as_ref() {
                    "lvl" => {
                        if let (Some(aid), Some((ilvl, l))) = (abstract_id, level.take()) {
                            let levels = numbering.abstracts.entry(aid).or_default();
                            let ilvl = ilvl as usize;
                            if levels.len() <= ilvl {
                                levels.resize(ilvl + 1, Level::default());
                            }
                            levels[ilvl] = l;
                        }
                    }
                    "abstractNum" => abstract_id = None,
                    "num" => {
                        if let Some((id, aid, overrides)) = num.take() {
                            numbering.nums.insert(id, (aid, overrides));
                        }
                    }
                    "lvlOverride" => override_level = None,
                    _ => {}
                },
                Event::Eof => break,
                _ => {}
            }
        }
        Ok(numbering)
    }

    pub fn counter(&self) -> ListCounter<'_> {
        ListCounter {
            numbering: self,
            counters: HashMap::new(),
        }
    }
}

/// Walks paragraphs in document order and produces their list labels.
pub struct ListCounter<'a> {
    numbering: &'a Numbering,
    counters: HashMap<CounterKey, [Option<u32>; 9]>,
}

#[derive(Hash, PartialEq, Eq)]
enum CounterKey {
    Abstract(u32),
    Num(u32),
}

impl ListCounter<'_> {
    pub fn next_label(&mut self, num_id: u32, level: u8) -> Option<String> {
        if num_id == 0 {
            return None;
        }
        let (abstract_id, overrides) = self.numbering.nums.get(&num_id)?;
        let levels = self.numbering.abstracts.get(abstract_id)?;
        let level = (level as usize).min(8);
        let def = levels.get(level)?;
        // Lists that restart numbering get their own counters; others continue
        // the sequence shared by every list built from the same abstract definition.
        let key = if overrides.is_empty() {
            CounterKey::Abstract(*abstract_id)
        } else {
            CounterKey::Num(num_id)
        };
        let start_of = |l: usize| {
            overrides
                .get(&(l as u8))
                .copied()
                .or_else(|| levels.get(l).map(|d| d.start))
                .unwrap_or(1)
        };
        let counters = self.counters.entry(key).or_insert([None; 9]);
        counters[level] = Some(counters[level].map_or(start_of(level), |c| c + 1));
        for deeper in counters.iter_mut().skip(level + 1) {
            *deeper = None;
        }
        if def.format == "none" {
            return (!def.text.is_empty()).then(|| def.text.clone());
        }
        if def.format == "bullet" {
            return Some(bullet(&def.text));
        }
        let mut label = String::new();
        let mut chars = def.text.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '%'
                && let Some(d) = chars.peek().and_then(|d| d.to_digit(10))
            {
                chars.next();
                let l = (d as usize).saturating_sub(1).min(8);
                let value = counters[l].unwrap_or_else(|| start_of(l));
                let format = levels.get(l).map_or("decimal", |d| d.format.as_str());
                label.push_str(&format_number(value, format));
                continue;
            }
            label.push(c);
        }
        Some(label)
    }
}

fn bullet(text: &str) -> String {
    // Symbol/Wingdings bullets live in the private use area; show a plain dot.
    match text.chars().next() {
        Some(c) if ('\u{E000}'..='\u{F8FF}').contains(&c) => "•".to_string(),
        Some(_) => text.to_string(),
        None => "•".to_string(),
    }
}

pub fn format_number(n: u32, format: &str) -> String {
    match format {
        "decimalZero" => format!("{n:02}"),
        "upperLetter" => letters(n).to_uppercase(),
        "lowerLetter" => letters(n),
        "upperRoman" => roman(n),
        "lowerRoman" => roman(n).to_lowercase(),
        "chineseCounting"
        | "chineseCountingThousand"
        | "japaneseCounting"
        | "taiwaneseCounting"
        | "taiwaneseCountingThousand"
        | "ideographDigital" => chinese(n, false),
        "chineseLegalSimplified" => chinese(n, true),
        "ideographTraditional" => cycle(n, "甲乙丙丁戊己庚辛壬癸"),
        "ideographZodiac" => cycle(n, "子丑寅卯辰巳午未申酉戌亥"),
        "decimalEnclosedCircle" | "decimalEnclosedCircleChinese" if (1..=20).contains(&n) => {
            char::from_u32(0x2460 + n - 1).unwrap().to_string()
        }
        "decimalFullWidth" | "decimalFullWidth2" => n
            .to_string()
            .chars()
            .map(|c| char::from_u32(c as u32 - '0' as u32 + 0xFF10).unwrap())
            .collect(),
        _ => n.to_string(),
    }
}

fn letters(n: u32) -> String {
    // Word repeats the letter: a..z, aa..zz, aaa..
    if n == 0 {
        return String::new();
    }
    let letter = (b'a' + ((n - 1) % 26) as u8) as char;
    std::iter::repeat_n(letter, ((n - 1) / 26 + 1) as usize).collect()
}

fn roman(mut n: u32) -> String {
    const TABLE: [(u32, &str); 13] = [
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ];
    let mut out = String::new();
    for (value, s) in TABLE {
        while n >= value {
            out.push_str(s);
            n -= value;
        }
    }
    out
}

fn cycle(n: u32, set: &str) -> String {
    let chars: Vec<char> = set.chars().collect();
    chars[((n.max(1) - 1) as usize) % chars.len()].to_string()
}

fn chinese(n: u32, legal: bool) -> String {
    let digits: Vec<char> = if legal {
        "零壹贰叁肆伍陆柒捌玖"
    } else {
        "〇一二三四五六七八九"
    }
    .chars()
    .collect();
    let units: [&str; 4] = if legal {
        ["", "拾", "佰", "仟"]
    } else {
        ["", "十", "百", "千"]
    };
    if n == 0 {
        return digits[0].to_string();
    }
    if n >= 10000 {
        return n.to_string();
    }
    let ds: Vec<u32> = n
        .to_string()
        .chars()
        .map(|c| c.to_digit(10).unwrap())
        .collect();
    let len = ds.len();
    let mut out = String::new();
    let mut pending_zero = false;
    for (i, d) in ds.iter().enumerate() {
        let unit = units[len - 1 - i];
        if *d == 0 {
            pending_zero = !out.is_empty();
            continue;
        }
        if pending_zero {
            out.push('零');
            pending_zero = false;
        }
        // 10–19 read as 十, 十一 … rather than 一十, 一十一.
        if !(legal || *d != 1 || len != 2 || i != 0) {
            out.push_str(unit);
            continue;
        }
        out.push(digits[*d as usize]);
        out.push_str(unit);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_numbers() {
        assert_eq!(format_number(1, "chineseCounting"), "一");
        assert_eq!(format_number(10, "chineseCounting"), "十");
        assert_eq!(format_number(12, "chineseCounting"), "十二");
        assert_eq!(format_number(20, "chineseCountingThousand"), "二十");
        assert_eq!(format_number(105, "chineseCountingThousand"), "一百零五");
        assert_eq!(format_number(3, "chineseLegalSimplified"), "叁");
        assert_eq!(format_number(3, "ideographTraditional"), "丙");
        assert_eq!(format_number(4, "upperRoman"), "IV");
        assert_eq!(format_number(28, "lowerLetter"), "bb");
        assert_eq!(format_number(2, "decimalEnclosedCircle"), "②");
        assert_eq!(format_number(7, "decimal"), "7");
    }

    #[test]
    fn counts_multi_level_lists() {
        let xml = r#"<w:numbering xmlns:w="w">
          <w:abstractNum w:abstractNumId="0">
            <w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="chineseCounting"/><w:lvlText w:val="%1、"/></w:lvl>
            <w:lvl w:ilvl="1"><w:start w:val="1"/><w:numFmt w:val="chineseCounting"/><w:lvlText w:val="（%2）"/></w:lvl>
            <w:lvl w:ilvl="2"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%1.%2.%3"/></w:lvl>
          </w:abstractNum>
          <w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num>
        </w:numbering>"#;
        let numbering = Numbering::parse(xml).unwrap();
        let mut c = numbering.counter();
        assert_eq!(c.next_label(1, 0).as_deref(), Some("一、"));
        assert_eq!(c.next_label(1, 1).as_deref(), Some("（一）"));
        assert_eq!(c.next_label(1, 1).as_deref(), Some("（二）"));
        assert_eq!(c.next_label(1, 2).as_deref(), Some("一.二.1"));
        assert_eq!(c.next_label(1, 0).as_deref(), Some("二、"));
        assert_eq!(c.next_label(1, 1).as_deref(), Some("（一）"));
        assert_eq!(c.next_label(0, 0), None);
        assert_eq!(c.next_label(99, 0), None);
    }
}
