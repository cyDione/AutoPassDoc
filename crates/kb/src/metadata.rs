//! Best-effort extraction of 文号, 发文机关, 成文日期 and title from the text
//! of an official document. Every field is optional; the user can correct them.

use std::sync::LazyLock;

use regex::Regex;

use crate::DocMeta;
use crate::text::{is_han, normalize, parse_number};

/// 文号 in width-normalised text: `国办发〔2024〕12号`, `京政发[2023]5号`,
/// `财预【2022】1号`, `X发(2021)3号`.
static DOC_NUMBER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(\p{Han}{0,12})\s*[〔\[【(]\s*((?:19|20)[0-9]{2})\s*[〕\]】)]\s*第?\s*([0-9]{1,5})\s*号",
    )
    .unwrap()
});

static DATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"([0-9]{4}|[〇○零OoΟ一二三四五六七八九]{4})\s*年\s*",
        r"([0-9]{1,2}|[一二三四五六七八九十]{1,3})\s*月\s*",
        r"([0-9]{1,2}|[一二三四五六七八九十]{1,4})\s*日"
    ))
    .unwrap()
});

static ISSUER_SUFFIX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(人民政府|政府|办公厅|办公室|委员会|委|部|厅|局|院|署|中心|公司|银行|集团|会|处)$")
        .unwrap()
});

/// "发布日期：2024年1月20日", as on government websites.
static DATE_LABEL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(发布|成文|发文|印发)(日期|时间)\s*:").unwrap());

static SEAL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*\([^)]{0,4}章\)$").unwrap());

static TITLE_SUFFIX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        "(通知|意见|办法|规定|方案|报告|决定|批复|函|通报|公告|通告|条例|细则|规则|规划|计划|纲要|",
        "指引|指南|要点|请示|决议|纪要|说明|规范|制度|法|令|总结|讲话|标准|清单|目录|章程|守则|须知|答复|建议)",
        r"(\([^)]{1,15}\))?》?$"
    ))
    .unwrap()
});

/// Characters after which a run of Han characters before a 文号 cannot
/// continue into the issuing organ's code ("根据国办发…" → "国办发").
const PREFIX_BREAKS: &[char] = &[
    '的', '号', '和', '及', '与', '或', '见', '将', '据', '照', '按', '依', '即', '在', '为', '由',
    '是', '了', '对',
];
const PREFIX_LEADING_WORDS: &[&str] = &[
    "根据", "依据", "按照", "参照", "遵照", "对照", "贯彻", "落实", "执行", "印发", "转发", "现将",
    "关于", "通知", "文件", "精神", "要求", "规定", "办法", "附件",
];

/// A 文号 found in width-normalised text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DocNumberMatch {
    /// Byte range in the searched text, starting at the cleaned prefix.
    pub start: usize,
    pub end: usize,
    /// Canonical form, e.g. `国办发〔2024〕12号`.
    pub normalized: String,
    /// Canonical form with the uncleaned run of Han characters in front.
    pub with_context: String,
}

/// Finds 文号 in text that went through [`normalize`].
pub(crate) fn find_doc_numbers(norm: &str) -> Vec<DocNumberMatch> {
    DOC_NUMBER
        .captures_iter(norm)
        .filter_map(|c| {
            let raw = c.get(1).unwrap();
            let prefix = clean_prefix(raw.as_str());
            if prefix.is_empty() {
                return None;
            }
            let number: u32 = c[3].parse().ok()?;
            let rest = format!("〔{}〕{number}号", &c[2]);
            Some(DocNumberMatch {
                start: raw.end() - prefix.len(),
                end: c.get(0).unwrap().end(),
                normalized: format!("{prefix}{rest}"),
                with_context: format!("{}{rest}", raw.as_str()),
            })
        })
        .collect()
}

fn clean_prefix(raw: &str) -> &str {
    let cut = raw
        .char_indices()
        .rfind(|(_, c)| PREFIX_BREAKS.contains(c))
        .map_or(0, |(i, c)| i + c.len_utf8());
    let mut prefix = &raw[cut..];
    while let Some(rest) = PREFIX_LEADING_WORDS
        .iter()
        .find_map(|w| prefix.strip_prefix(w))
    {
        prefix = rest;
    }
    prefix
}

/// Canonical form of the first 文号 in `s` (brackets become 〔〕), if any.
pub fn normalize_doc_number(s: &str) -> Option<String> {
    find_doc_numbers(&normalize(s))
        .into_iter()
        .next()
        .map(|m| m.normalized)
}

/// The first date in `s` (`2024年3月5日`, `二〇二四年三月五日`) as `YYYY-MM-DD`.
pub fn parse_date(s: &str) -> Option<String> {
    DATE.captures_iter(&normalize(s))
        .find_map(|c| date_from(&c[1], &c[2], &c[3]))
}

fn date_from(y: &str, m: &str, d: &str) -> Option<String> {
    let (y, m, d) = (parse_number(y)?, parse_number(m)?, parse_number(d)?);
    ((1900..=2100).contains(&y) && (1..=12).contains(&m) && (1..=31).contains(&d))
        .then(|| format!("{y:04}-{m:02}-{d:02}"))
}

struct Line<'a> {
    text: &'a str,
    norm: String,
    len: usize,
}

/// Extracts metadata from the lines (paragraphs) of a document.
pub fn extract_metadata<S: AsRef<str>>(lines: &[S]) -> DocMeta {
    let lines: Vec<Line> = lines
        .iter()
        .map(|l| l.as_ref().trim())
        .filter(|l| !l.is_empty())
        .map(|text| {
            let norm = normalize(text);
            Line {
                text,
                len: norm.chars().count(),
                norm,
            }
        })
        .collect();
    let head = &lines[..lines.len().min(15)];

    let doc_number = head.iter().find_map(|l| {
        let m = find_doc_numbers(&l.norm).into_iter().next()?;
        let outside = l.norm[..m.start].chars().count() + l.norm[m.end..].chars().count();
        (outside <= 12).then_some(m.normalized)
    });

    let title = find_title(head);

    let signature = find_signature_date(&lines);
    let date = signature
        .as_ref()
        .map(|s| s.1.clone())
        .or_else(|| find_top_date(head));

    let issuer = signature
        .and_then(|(i, _, rest)| {
            if issuer_like(&rest) {
                return Some(clean_issuer(&rest));
            }
            let names: Vec<String> = lines[..i]
                .iter()
                .rev()
                .skip_while(|l| SEAL.replace(&l.norm, "").trim().is_empty())
                .take(4)
                .take_while(|l| issuer_like(&l.norm))
                .map(|l| clean_issuer(l.text))
                .collect();
            (!names.is_empty()).then(|| names.into_iter().rev().collect::<Vec<_>>().join(" "))
        })
        .or_else(|| {
            head.iter().take(3).find_map(|l| {
                let name = l.norm.strip_suffix("文件")?.trim();
                (l.len <= 30 && !name.is_empty()).then(|| clean_issuer(name))
            })
        })
        .or_else(|| {
            let before_title = title.as_ref().map_or(0, |t| t.0).min(5);
            head[..before_title]
                .iter()
                .find(|l| issuer_like(&l.norm))
                .map(|l| clean_issuer(l.text))
        })
        .or_else(|| {
            let (_, title) = title.as_ref()?;
            let (organ, _) = title.split_once("关于")?;
            issuer_like(&normalize(organ)).then(|| clean_issuer(organ))
        });

    DocMeta {
        title: title.map(|t| t.1),
        doc_number,
        issuer,
        date,
    }
}

/// The red letterhead of an official document ("上海市人民政府文件"), which
/// is not its title.
pub fn is_letterhead(title: &str) -> bool {
    let norm = normalize(title.trim());
    let len = norm.chars().count();
    len <= 30 && (norm.ends_with("文件") || !find_doc_numbers(&norm).is_empty())
}

/// Header lines that are not the title: red header, 文号, date, 份号, 密级.
fn is_header_noise(l: &Line) -> bool {
    const MARKS: &[&str] = &[
        "签发人",
        "密级",
        "特急",
        "加急",
        "特提",
        "平急",
        "机密",
        "秘密",
        "绝密",
        "份号",
        "★",
    ];
    (l.len <= 30 && (l.norm.ends_with("文件") || !find_doc_numbers(&l.norm).is_empty()))
        || (l.len <= 20 && DATE.is_match(&l.norm))
        || MARKS.iter().any(|m| l.norm.starts_with(m))
        || !l.norm.chars().any(is_han)
}

/// Index of the title's (first) line and the title text.
fn find_title(head: &[Line]) -> Option<(usize, String)> {
    // Lines up to the salutation ("各区人民政府：") that could be a title.
    let end = head
        .iter()
        .position(|l| l.norm.ends_with(':'))
        .unwrap_or(head.len());
    let candidates = || (0..end).filter(|&i| !is_header_noise(&head[i]));

    for i in candidates() {
        let l = &head[i];
        if !l.norm.contains("关于") || l.len > 80 {
            continue;
        }
        if TITLE_SUFFIX.is_match(&l.norm) {
            return Some((i, l.text.to_string()));
        }
        // A long title wraps: "国务院办公厅关于印发《…》" / "的通知".
        if let Some(next) = head.get(i + 1).filter(|n| i + 1 < end && n.len <= 30)
            && TITLE_SUFFIX.is_match(&format!("{}{}", l.norm, next.norm))
        {
            return Some((i, format!("{}{}", l.text, next.text)));
        }
    }
    let short = |l: &Line| l.len <= 50 && !l.norm.contains(['。', ',', ';']);
    candidates()
        .find(|&i| short(&head[i]) && TITLE_SUFFIX.is_match(&head[i].norm))
        .or_else(|| candidates().find(|&i| short(&head[i])))
        .map(|i| (i, head[i].text.to_string()))
}

/// The last line that is a date alone, or a date after the issuer's name:
/// (line index, date, text of the line besides the date).
fn find_signature_date(lines: &[Line]) -> Option<(usize, String, String)> {
    lines.iter().enumerate().rev().find_map(|(i, l)| {
        // The 版记 line "××办公厅 2024年3月6日印发" is the printing date.
        if l.norm.contains("印发") || l.norm.starts_with("抄送") || l.norm.starts_with("抄报")
        {
            return None;
        }
        let c = DATE.captures_iter(&l.norm).last()?;
        let m = c.get(0).unwrap();
        let rest = format!("{}{}", &l.norm[..m.start()], &l.norm[m.end()..]);
        let rest = rest.trim().trim_matches(['(', ')']).trim().to_string();
        if !rest.is_empty() && !issuer_like(&rest) {
            return None;
        }
        Some((i, date_from(&c[1], &c[2], &c[3])?, rest))
    })
}

/// A date under the title, e.g. "（2021年6月10日第十三届全国人民代表大会常务委员会第二十九次会议通过）"
/// or "发布日期：2024年1月20日".
fn find_top_date(head: &[Line]) -> Option<String> {
    head.iter().take(8).find_map(|l| {
        let c = DATE.captures(&l.norm)?;
        let alone = c.get(0).unwrap().as_str().chars().count() + 2 >= l.len;
        (alone || l.norm.starts_with('(') || DATE_LABEL.is_match(&l.norm))
            .then(|| date_from(&c[1], &c[2], &c[3]))?
    })
}

fn issuer_like(norm: &str) -> bool {
    let name = SEAL.replace(norm.trim(), "");
    let len = name.chars().count();
    (2..=40).contains(&len)
        && !name.starts_with('各')
        && !name.contains([':', ',', '。', ';', '!', '?'])
        && !name.chars().any(|c| c.is_ascii_digit())
        && !["关于", "通知", "印发", "如下"]
            .iter()
            .any(|w| name.contains(w))
        && ISSUER_SUFFIX.is_match(&name)
}

fn clean_issuer(s: &str) -> String {
    let norm = normalize(s);
    let name = SEAL.replace(norm.trim(), "");
    // Keep the original characters, only dropping the seal note and extra spaces.
    let kept: String = s.trim().chars().take(name.chars().count()).collect();
    kept.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_doc_numbers_in_context() {
        let m = find_doc_numbers(&normalize("根据国办发〔2024〕12号文件和京政发[2023]005号"));
        let found: Vec<&str> = m.iter().map(|m| m.normalized.as_str()).collect();
        assert_eq!(found, ["国办发〔2024〕12号", "京政发〔2023〕5号"]);
        assert_eq!(
            normalize_doc_number("财预【2022】1号"),
            Some("财预〔2022〕1号".into())
        );
        assert_eq!(
            normalize_doc_number("财税（2021）8号"),
            Some("财税〔2021〕8号".into())
        );
        assert_eq!(normalize_doc_number("〔2024〕3号"), None);
    }

    #[test]
    fn parses_dates() {
        assert_eq!(parse_date("2024年3月5日"), Some("2024-03-05".into()));
        assert_eq!(parse_date("二〇二四年三月五日"), Some("2024-03-05".into()));
        assert_eq!(
            parse_date("二○二三年十二月三十一日"),
            Some("2023-12-31".into())
        );
        assert_eq!(
            parse_date("２０２２年１月１０日"),
            Some("2022-01-10".into())
        );
        assert_eq!(parse_date("2024年13月5日"), None);
    }
}
