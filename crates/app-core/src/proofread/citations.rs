//! The list of documents a report cites: titles in 《》, standard numbers
//! (`GB/T 50378-2019`, `DG/TJ08-2012-2018`) and document numbers
//! (`沪府办发〔2023〕5号`).

use std::sync::LazyLock;

use regex::Regex;

use super::{Citation, CitationKind, ProofParagraph};

static TITLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"《([^《》\n]{2,80})》").unwrap());

/// Standard codes, longest first so `GB/T` wins over `GB`.
const STANDARD_CODES: &str = concat!(
    r"GB/T|GB/Z|GBZ/T|GBZ|GB|JGJ/T|JGJ|CJJ/T|CJJ|CJ/T|CJ|HJ/T|HJ|DB\d{2}/T|DB\d{2}|",
    r"DG/TJ\s?08|DGJ\s?08|DBJ\s?\d{2}|T/[A-Z]{2,10}|CECS|JTG/T|JTG|JT/T|SL/T|SL|DL/T|DL|NB/T|",
    r"GA/T|GA|JB/T|HG/T|YB/T|SH/T|SY/T|TB/T|TB|QX/T|WS/T|WS|YY/T|JC/T|JG/T|LY/T|NY/T|SJ/T|",
    r"YD/T|GY/T|MH/T|JR/T|AQ/T|AQ|XF/T|XF|CH/T|DZ/T|TD/T|CQJTG/T"
);

static STANDARD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?:^|[^A-Za-z0-9/])((?:{STANDARD_CODES}))\s*[-—–－]?\s*(\d{{1,6}}(?:\.\d{{1,3}})?)(?:\s*[-—–－]\s*((?:19|20)\d{{2}}))?"
    ))
    .unwrap()
});

static DOC_NUMBER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"\p{Han}{1,12}\s*[〔\[【(（［]\s*(?:19|20)\d{2}\s*[〕\]】)）］]\s*第?\s*\d{1,5}\s*号",
    )
    .unwrap()
});

/// Full-width letters, digits and slashes as ASCII, one char for one char.
fn narrow(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '\u{FF01}'..='\u{FF5E}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
            c => c,
        })
        .collect()
}

/// A standard number found in text: byte range in the narrowed text and
/// the canonical form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StandardMatch {
    pub start: usize,
    pub end: usize,
    pub number: String,
}

/// Standard numbers in `text`, as `GB/T 50378-2019`; local Shanghai codes
/// that end in digits (`DG/TJ08`) are joined with a hyphen.
pub fn standard_numbers(text: &str) -> Vec<StandardMatch> {
    let text = narrow(text);
    STANDARD
        .captures_iter(&text)
        .map(|c| {
            let code: String = c[1].chars().filter(|c| !c.is_whitespace()).collect();
            let sep = if code.ends_with(|c: char| c.is_ascii_digit()) {
                "-"
            } else {
                " "
            };
            let mut number = format!("{code}{sep}{}", &c[2]);
            if let Some(y) = c.get(3) {
                number.push('-');
                number.push_str(y.as_str());
            }
            StandardMatch {
                start: c.get(1).unwrap().start(),
                end: c.get(0).unwrap().end(),
                number,
            }
        })
        .collect()
}

/// 文号 in `text`, canonical (`国办发〔2024〕12号`), with byte ranges.
pub fn doc_numbers(text: &str) -> Vec<(usize, usize, String)> {
    DOC_NUMBER
        .find_iter(text)
        .filter_map(|m| {
            let n = kb::metadata::normalize_doc_number(m.as_str())?;
            // The prefix may have been cleaned ("根据国发…" → "国发…").
            let prefix: String = n.chars().take_while(|&c| c != '〔').collect();
            let start = m.start() + m.as_str().find(&prefix).unwrap_or(0);
            Some((start, m.end(), n))
        })
        .collect()
}

/// What kind of document a title names.
pub fn classify(title: &str, standard: bool, doc_no: bool) -> CitationKind {
    let has = |words: &[&str]| words.iter().any(|w| title.contains(w));
    if standard
        || has(&[
            "标准",
            "规范",
            "规程",
            "导则",
            "技术规定",
            "技术要求",
            "设计规定",
            "图集",
        ])
    {
        return CitationKind::Standard;
    }
    if title.ends_with('法')
        || title.contains("条例")
        || title.contains("法（")
        || title.starts_with("中华人民共和国")
    {
        return CitationKind::Law;
    }
    if ["规定", "办法", "细则", "规则"]
        .iter()
        .any(|w| title.ends_with(w) || title.contains(&format!("{w}（")))
    {
        return if doc_no {
            CitationKind::Policy
        } else {
            CitationKind::Law
        };
    }
    if has(&[
        "通知", "意见", "方案", "规划", "纲要", "计划", "决定", "指南", "指引", "公告", "通告",
        "批复", "要点", "措施", "行动", "政策",
    ]) || doc_no
    {
        return CitationKind::Policy;
    }
    CitationKind::Unknown
}

fn key(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

fn add(out: &mut Vec<Citation>, c: Citation) {
    let k = key(&c.title);
    let found = out.iter_mut().find(|o| {
        key(&o.title) == k
            || c.standard_no.is_some() && o.standard_no == c.standard_no
            || c.doc_no.is_some() && o.doc_no == c.doc_no && c.title == c.doc_no.clone().unwrap()
    });
    match found {
        Some(o) => {
            for p in c.paragraphs {
                if !o.paragraphs.contains(&p) {
                    o.paragraphs.push(p);
                }
            }
            if o.standard_no.is_none() {
                o.standard_no = c.standard_no;
            }
            if o.doc_no.is_none() {
                o.doc_no = c.doc_no;
            }
            if o.kind == CitationKind::Unknown {
                o.kind = c.kind;
            }
        }
        None => out.push(c),
    }
}

/// Every document the paragraphs cite, merged by title (or number), in
/// order of first mention.
pub fn extract_citations(paras: &[ProofParagraph]) -> Vec<Citation> {
    let mut out: Vec<Citation> = Vec::new();
    let mut loose: Vec<Citation> = Vec::new();
    for p in paras {
        let text = narrow(&p.text);
        let standards = standard_numbers(&text);
        let docs = doc_numbers(&text);
        let mut used_std = vec![false; standards.len()];
        let mut used_doc = vec![false; docs.len()];
        for m in TITLE.captures_iter(&text) {
            let whole = m.get(0).unwrap();
            let title = m[1].trim().to_string();
            // A number inside the title, right after it (optionally in
            // brackets) or right before it.
            let near = |start: usize, end: usize| {
                let inside = start >= whole.start() && end <= whole.end();
                let after = start >= whole.end()
                    && text[whole.end()..start]
                        .chars()
                        .all(|c| c.is_whitespace() || matches!(c, '（' | '(' | '：' | ':'))
                    && text[whole.end()..start].chars().count() <= 2;
                let before = end <= whole.start()
                    && text[end..whole.start()].chars().all(char::is_whitespace);
                inside || after || before
            };
            let std_i = standards.iter().position(|s| near(s.start, s.end));
            let doc_i = docs.iter().position(|d| near(d.0, d.1));
            if let Some(i) = std_i {
                used_std[i] = true;
            }
            if let Some(i) = doc_i {
                used_doc[i] = true;
            }
            let standard_no = std_i.map(|i| standards[i].number.clone());
            let doc_no = doc_i.map(|i| docs[i].2.clone());
            let kind = classify(&title, standard_no.is_some(), doc_no.is_some());
            add(
                &mut out,
                Citation {
                    title,
                    kind,
                    standard_no,
                    doc_no,
                    paragraphs: vec![p.index],
                },
            );
        }
        for (s, _) in standards.iter().zip(&used_std).filter(|(_, u)| !**u) {
            loose.push(Citation {
                title: s.number.clone(),
                kind: CitationKind::Standard,
                standard_no: Some(s.number.clone()),
                doc_no: None,
                paragraphs: vec![p.index],
            });
        }
        for (d, _) in docs.iter().zip(&used_doc).filter(|(_, u)| !**u) {
            loose.push(Citation {
                title: d.2.clone(),
                kind: CitationKind::Policy,
                standard_no: None,
                doc_no: Some(d.2.clone()),
                paragraphs: vec![p.index],
            });
        }
    }
    for c in loose {
        add(&mut out, c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paras(texts: &[&str]) -> Vec<ProofParagraph> {
        texts
            .iter()
            .enumerate()
            .map(|(i, t)| ProofParagraph::new(i, *t))
            .collect()
    }

    #[test]
    fn standard_number_forms() {
        let n = |s: &str| {
            standard_numbers(s)
                .into_iter()
                .map(|m| m.number)
                .collect::<Vec<_>>()
        };
        assert_eq!(n("执行GB/T 50378-2019标准"), ["GB/T 50378-2019"]);
        assert_eq!(n("按照GB 50016—2014（2018年版）"), ["GB 50016-2014"]);
        assert_eq!(n("《室外排水设计标准》GB50014-2021"), ["GB 50014-2021"]);
        assert_eq!(
            n("DG/TJ 08-2012-2018及DB31/T 1234-2020"),
            ["DG/TJ08-2012-2018", "DB31/T 1234-2020"]
        );
        assert_eq!(n("ＧＢ／Ｔ ５０３７８－２０１９"), ["GB/T 50378-2019"]);
        assert_eq!(
            n("JGJ/T 98-2010、CJJ 1-2008、HJ 2.2-2018"),
            ["JGJ/T 98-2010", "CJJ 1-2008", "HJ 2.2-2018"]
        );
        assert_eq!(n("T/CECS 123-2020"), ["T/CECS 123-2020"]);
        // Memory sizes are not standards.
        assert!(n("配置32GB 内存、128GB 硬盘").is_empty());
        assert!(n("8GB 128").is_empty());
    }

    #[test]
    fn doc_number_forms() {
        let d: Vec<String> = doc_numbers("根据国发〔2024〕12号和沪府办发[2023]5号文件")
            .into_iter()
            .map(|d| d.2)
            .collect();
        assert_eq!(d, ["国发〔2024〕12号", "沪府办发〔2023〕5号"]);
    }

    #[test]
    fn classifies() {
        use CitationKind::*;
        assert_eq!(classify("中华人民共和国水法", false, false), Law);
        assert_eq!(classify("上海市排水与污水处理条例", false, false), Law);
        assert_eq!(classify("建设项目环境保护管理条例", false, false), Law);
        assert_eq!(
            classify("上海市建设工程招标投标管理办法", false, false),
            Law
        );
        assert_eq!(classify("关于加强管理的若干规定", false, true), Policy);
        assert_eq!(classify("室外排水设计标准", false, false), Standard);
        assert_eq!(classify("城镇污水处理厂污染物排放", true, false), Standard);
        assert_eq!(
            classify("上海市城市总体规划（2017—2035年）", false, false),
            Policy
        );
        assert_eq!(classify("关于印发《……》的通知", false, false), Policy);
        assert_eq!(classify("崇明世界级生态岛发展", false, false), Unknown);
    }

    #[test]
    fn extracts_and_merges() {
        let cs = extract_citations(&paras(&[
            "依据《中华人民共和国水污染防治法》和《室外排水设计标准》（GB 50014-2021）。",
            "管网设计执行《室外排水设计标准》GB 50014-2021，并参照DG/TJ 08-2012-2018。",
            "根据《关于加快推进城镇污水处理设施建设的通知》（沪府办发〔2023〕5号）要求。",
            "同时满足GB 50014-2021和沪府办发〔2023〕5号的规定。",
        ]));
        let summary: Vec<_> = cs
            .iter()
            .map(|c| {
                (
                    c.title.as_str(),
                    c.kind,
                    c.standard_no.as_deref(),
                    c.doc_no.as_deref(),
                    c.paragraphs.clone(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                (
                    "中华人民共和国水污染防治法",
                    CitationKind::Law,
                    None,
                    None,
                    vec![0]
                ),
                (
                    "室外排水设计标准",
                    CitationKind::Standard,
                    Some("GB 50014-2021"),
                    None,
                    vec![0, 1, 3]
                ),
                (
                    "关于加快推进城镇污水处理设施建设的通知",
                    CitationKind::Policy,
                    None,
                    Some("沪府办发〔2023〕5号"),
                    vec![2, 3]
                ),
                (
                    "DG/TJ08-2012-2018",
                    CitationKind::Standard,
                    Some("DG/TJ08-2012-2018"),
                    None,
                    vec![1]
                ),
            ]
        );
    }
}
