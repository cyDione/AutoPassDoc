//! Rule checks: format, numbering and place names. Pure functions over the
//! paragraphs; offsets are in characters of the editable text.

use std::collections::HashMap;

use super::divisions::{self, Division, Level};
use super::facts;
use super::{Category, Issue, ProjectFacts, ProofParagraph, Severity};

pub fn is_han(c: char) -> bool {
    matches!(c as u32,
        0x4E00..=0x9FFF | 0x3400..=0x4DBF | 0xF900..=0xFAFF | 0x20000..=0x2FA1F | 0x3007)
}

/// Full-width punctuation used in Chinese text.
pub fn is_cjk_punct(c: char) -> bool {
    matches!(
        c,
        '，' | '。'
            | '、'
            | '；'
            | '：'
            | '？'
            | '！'
            | '“'
            | '”'
            | '‘'
            | '’'
            | '（'
            | '）'
            | '《'
            | '》'
            | '〈'
            | '〉'
            | '【'
            | '】'
            | '〔'
            | '〕'
            | '「'
            | '」'
            | '『'
            | '』'
            | '…'
            | '—'
            | '·'
            | '～'
    )
}

fn cjkish(c: char) -> bool {
    is_han(c) || is_cjk_punct(c)
}

/// Spaces that are never needed inside Chinese text (the full-width space
/// is left alone: it is used for indents and aligning labels).
fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\u{a0}')
}

fn is_blank(text: &str) -> bool {
    text.trim().is_empty()
}

// ---------------------------------------------------------------------------
// Format

/// Format findings: stray spaces, half-width punctuation in Chinese
/// sentences, repeated punctuation, unbalanced brackets and quotes, and runs
/// of empty paragraphs.
pub fn check_format(paras: &[ProofParagraph]) -> Vec<Issue> {
    let mut out = Vec::new();
    let mut empty_run: Vec<usize> = Vec::new();
    let flush = |run: &mut Vec<usize>, out: &mut Vec<Issue>| {
        if run.len() >= 2 {
            out.push(Issue::span(
                Category::Format,
                Severity::Warning,
                run[1],
                &[],
                0,
                0,
                None,
                format!(
                    "连续 {} 个空段落，建议只保留一个，或改用段前、段后间距",
                    run.len()
                ),
            ));
        }
        run.clear();
    };
    for p in paras {
        if p.in_table {
            flush(&mut empty_run, &mut out);
            continue;
        }
        if is_blank(&p.text) {
            if empty_run.last().is_some_and(|&last| last + 1 != p.index) {
                flush(&mut empty_run, &mut out);
            }
            empty_run.push(p.index);
            continue;
        }
        flush(&mut empty_run, &mut out);
        out.extend(format_paragraph(p));
    }
    flush(&mut empty_run, &mut out);
    out
}

fn format_paragraph(p: &ProofParagraph) -> Vec<Issue> {
    let chars: Vec<char> = p.text.chars().collect();
    let n = chars.len();
    let mut out = Vec::new();
    let mut add = |start: usize, end: usize, sev: Severity, sug: Option<&str>, reason: &str| {
        out.push(Issue::span(
            Category::Format,
            sev,
            p.index,
            &chars,
            start,
            end,
            sug.map(str::to_string),
            reason,
        ));
    };

    let lead = chars.iter().take_while(|c| c.is_whitespace()).count();
    let trail = chars[lead..]
        .iter()
        .rev()
        .take_while(|c| c.is_whitespace())
        .count();
    if lead > 0 && chars[..lead].iter().any(|&c| c != '\u{3000}') {
        add(
            0,
            lead,
            Severity::Warning,
            Some(""),
            "段首有多余空格，缩进请用首行缩进设置",
        );
    }
    if trail > 0 {
        add(n - trail, n, Severity::Warning, Some(""), "段尾有多余空格");
    }

    let prefix = parse_prefix(&p.text, p.heading_level.is_some());
    let prefix_end = prefix.as_ref().map(|x| x.end);
    let chapter_like = prefix.as_ref().is_some_and(|x| {
        matches!(
            x.family,
            Family::Chapter | Family::Section | Family::Part | Family::Dotted(_)
        )
    });

    // Spaces inside the text.
    let body_end = n - trail;
    let mut i = lead;
    while i < body_end {
        if !is_space(chars[i]) {
            i += 1;
            continue;
        }
        let s = i;
        while i < body_end && is_space(chars[i]) {
            i += 1;
        }
        let (prev, next) = (chars[s - 1], chars[i]);
        let both_cjk = cjkish(prev) && cjkish(next);
        let after_heading_no = prefix_end == Some(s) && chapter_like;
        if i - s >= 2 {
            let sug = if both_cjk && !after_heading_no {
                ""
            } else {
                " "
            };
            add(s, i, Severity::Warning, Some(sug), "连续多个空格");
        } else if both_cjk && !after_heading_no {
            add(s, i, Severity::Warning, Some(""), "中文之间有多余空格");
        }
    }

    // Half-width punctuation in Chinese sentences.
    for i in lead..body_end {
        let c = chars[i];
        let full = match c {
            ',' => '，',
            ';' => '；',
            ':' => '：',
            '?' => '？',
            '!' => '！',
            _ => continue,
        };
        if i == 0 {
            continue;
        }
        let prev = chars[i - 1];
        let mut end = i + 1;
        while end < body_end && is_space(chars[end]) {
            end += 1;
        }
        let next = chars.get(end).copied();
        let prev_cjk = is_han(prev) || matches!(prev, '）' | '”' | '》' | '】');
        let next_ok = match next {
            None => true,
            Some(c) => cjkish(c) || c.is_ascii_digit() && full != '，',
        };
        if prev_cjk && next_ok {
            let sug = full.to_string();
            add(
                i,
                end,
                Severity::Warning,
                Some(&sug),
                "中文句子中使用了半角标点",
            );
        }
    }
    // Half-width parentheses around Chinese text.
    let numbering = prefix.as_ref().map(|x| x.start..x.end);
    let mut i = lead;
    while i < body_end {
        if chars[i] != '(' || numbering.as_ref().is_some_and(|r| r.contains(&i)) {
            i += 1;
            continue;
        }
        let close = (i + 1..body_end.min(i + 80))
            .take_while(|&j| chars[j] != '(')
            .find(|&j| chars[j] == ')');
        if let Some(j) = close
            && chars[i + 1..j].iter().any(|&c| is_han(c))
        {
            add(
                i,
                i + 1,
                Severity::Warning,
                Some("（"),
                "中文内容使用了半角括号",
            );
            add(
                j,
                j + 1,
                Severity::Warning,
                Some("）"),
                "中文内容使用了半角括号",
            );
            i = j + 1;
        } else {
            i += 1;
        }
    }

    // Repeated punctuation.
    const DUP: &[char] = &['，', '。', '、', '；', '：', '？', '！'];
    let mut i = lead;
    while i < body_end {
        if !DUP.contains(&chars[i]) {
            i += 1;
            continue;
        }
        let s = i;
        while i < body_end && DUP.contains(&chars[i]) {
            i += 1;
        }
        if i - s < 2 {
            continue;
        }
        let run = &chars[s..i];
        if run.iter().all(|&c| c == run[0]) {
            let sug = run[0].to_string();
            add(s, i, Severity::Error, Some(&sug), "标点重复");
        } else if !matches!(run, ['？', '！'] | ['！', '？']) {
            add(s, i, Severity::Warning, None, "标点连用，请保留一个");
        }
    }

    // Brackets and quotes.
    out.extend(balance(p.index, &chars, numbering));
    out
}

/// Unbalanced brackets and quotes, and pairs mixing full- and half-width
/// parentheses. The numbering token at the start (e.g. `1)`) is skipped.
fn balance(paragraph: usize, chars: &[char], skip: Option<std::ops::Range<usize>>) -> Vec<Issue> {
    const PAIRS: &[(&[char], &[char], &str)] = &[
        (&['（', '('], &['）', ')'], "括号"),
        (&['《'], &['》'], "书名号"),
        (&['【'], &['】'], "方括号"),
        (&['〔'], &['〕'], "六角括号"),
        (&['“'], &['”'], "双引号"),
        (&['‘'], &['’'], "单引号"),
    ];
    let mut out = Vec::new();
    let mut issue = |start: usize, sug: Option<&str>, sev: Severity, reason: String| {
        out.push(Issue::span(
            Category::Format,
            sev,
            paragraph,
            chars,
            start,
            start + 1,
            sug.map(str::to_string),
            reason,
        ));
    };
    for (opens, closes, name) in PAIRS {
        let mut stack: Vec<usize> = Vec::new();
        for (i, &c) in chars.iter().enumerate() {
            if skip.as_ref().is_some_and(|r| r.contains(&i)) {
                continue;
            }
            if opens.contains(&c) {
                if *name == "双引号" && !stack.is_empty() {
                    issue(
                        i,
                        None,
                        Severity::Warning,
                        "引号方向可能有误或未闭合".into(),
                    );
                    stack.pop();
                    continue;
                }
                stack.push(i);
            } else if closes.contains(&c) {
                match stack.pop() {
                    Some(o) if *name == "括号" && (chars[o] == '（') != (c == '）') => {
                        let han = chars[o + 1..i].iter().any(|&c| is_han(c));
                        let (at, sug) = match (chars[o] == '（', han) {
                            (true, true) => (i, "）"),
                            (true, false) => (o, "("),
                            (false, true) => (o, "（"),
                            (false, false) => (i, ")"),
                        };
                        issue(at, Some(sug), Severity::Warning, "括号全半角不一致".into());
                    }
                    Some(_) => {}
                    None => {
                        // "a)" / "1)" used as a list label inside a sentence.
                        let label = c == ')'
                            && i > 0
                            && chars[i - 1].is_ascii_alphanumeric()
                            && (i < 2 || !chars[i - 2].is_ascii_alphanumeric());
                        if !label {
                            issue(
                                i,
                                None,
                                Severity::Warning,
                                format!("{name}不配对：缺少前半个"),
                            );
                        }
                    }
                }
            }
        }
        for o in stack {
            issue(
                o,
                None,
                Severity::Warning,
                format!("{name}不配对：缺少后半个"),
            );
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Numbering

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Family {
    /// 第一章
    Chapter,
    /// 第一节
    Section,
    /// 第一篇 / 第一部分
    Part,
    /// 一、
    CnComma,
    /// （一）
    CnParen,
    /// 1. / 1、 / 1．
    Arabic,
    /// （1）
    ArabicParen,
    /// 1）
    ArabicHalf,
    /// ①
    Circled,
    /// 1.1 / 1.1.1 (depth = number of parts)
    Dotted(u8),
}

/// A numbering token typed at the start of a paragraph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prefix {
    pub family: Family,
    pub value: u32,
    /// Leading parts of a dotted number (`[2, 3]` for 2.3.4).
    pub parent: Vec<u32>,
    /// Bracket width ("full" / "half" / "mixed") or separator.
    pub variant: String,
    /// Character range of the whole token.
    pub start: usize,
    pub end: usize,
    /// Character range of the number inside the token.
    pub num_start: usize,
    pub num_end: usize,
    /// The number is written with Chinese numerals.
    pub chinese: bool,
}

const CN_DIGITS: &str = "〇一二三四五六七八九";

/// Chinese numerals up to 99: 一, 十, 十二, 二十, 九十九.
pub fn parse_cn(s: &str) -> Option<u32> {
    let digit = |c: char| match c {
        '两' => Some(2),
        '零' => Some(0),
        _ => CN_DIGITS.chars().position(|d| d == c).map(|p| p as u32),
    };
    let chars: Vec<char> = s.chars().collect();
    match chars.as_slice() {
        [c] if *c == '十' => Some(10),
        [c] => digit(*c).filter(|&d| d > 0),
        ['十', c] => digit(*c).filter(|&d| d > 0).map(|d| 10 + d),
        [c, '十'] => digit(*c).filter(|&d| d > 1).map(|d| d * 10),
        [a, '十', b] => Some(digit(*a).filter(|&d| d > 1)? * 10 + digit(*b).filter(|&d| d > 0)?),
        _ => None,
    }
}

pub fn render_cn(n: u32) -> String {
    let d = |i: u32| CN_DIGITS.chars().nth(i as usize).unwrap();
    match n {
        0 => "〇".into(),
        1..=9 => d(n).to_string(),
        10 => "十".into(),
        11..=19 => format!("十{}", d(n - 10)),
        _ if n.is_multiple_of(10) && n < 100 => format!("{}十", d(n / 10)),
        20..=99 => format!("{}十{}", d(n / 10), d(n % 10)),
        _ => n.to_string(),
    }
}

fn is_cn_numeral(c: char) -> bool {
    CN_DIGITS.contains(c) || c == '十' || c == '两' || c == '零'
}

/// Characters after a number that make it a quantity, not a list number:
/// 3.2亿元, 1.5万平方米.
const UNIT_CHARS: &str = "万亿千百元米倍个年月日次吨台套件项人户亩%公平立度层栋座条km";

/// The numbering typed at the start of `text`, if any.
pub fn parse_prefix(text: &str, is_heading: bool) -> Option<Prefix> {
    let chars: Vec<char> = text.chars().collect();
    let lead = chars.iter().take_while(|c| c.is_whitespace()).count();
    let rest = &chars[lead..];
    let at = |i: usize| rest.get(i).copied();
    let make = |family, value, parent, variant: &str, len, num: (usize, usize), chinese| {
        Some(Prefix {
            family,
            value,
            parent,
            variant: variant.to_string(),
            start: lead,
            end: lead + len,
            num_start: lead + num.0,
            num_end: lead + num.1,
            chinese,
        })
    };
    let first = at(0)?;

    // 第一章 / 第3节 / 第二部分
    if first == '第' {
        let len = rest[1..]
            .iter()
            .take_while(|c| is_cn_numeral(**c) || c.is_ascii_digit())
            .count();
        if len > 0 {
            let num: String = rest[1..1 + len].iter().collect();
            let (value, chinese) = match num.parse::<u32>() {
                Ok(v) => (v, false),
                Err(_) => (parse_cn(&num)?, true),
            };
            let tail: String = rest[1 + len..].iter().take(2).collect();
            let (family, unit_len) = match tail.chars().next() {
                Some('章') => (Family::Chapter, 1),
                Some('节') => (Family::Section, 1),
                Some('篇') => (Family::Part, 1),
                _ if tail == "部分" => (Family::Part, 2),
                _ => return None,
            };
            return make(
                family,
                value,
                vec![],
                "",
                1 + len + unit_len,
                (1, 1 + len),
                chinese,
            );
        }
        return None;
    }

    // （一） / (1)
    if first == '（' || first == '(' {
        let len = rest[1..]
            .iter()
            .take_while(|c| is_cn_numeral(**c) || c.is_ascii_digit())
            .count();
        let close = at(1 + len)?;
        if len == 0 || len > 3 || !(close == '）' || close == ')') {
            return None;
        }
        let num: String = rest[1..1 + len].iter().collect();
        let variant = match (first == '（', close == '）') {
            (true, true) => "full",
            (false, false) => "half",
            _ => "mixed",
        };
        return match num.parse::<u32>() {
            Ok(v) => make(
                Family::ArabicParen,
                v,
                vec![],
                variant,
                len + 2,
                (1, 1 + len),
                false,
            ),
            Err(_) => make(
                Family::CnParen,
                parse_cn(&num)?,
                vec![],
                variant,
                len + 2,
                (1, 1 + len),
                true,
            ),
        };
    }

    // ① … ⑳
    if ('①'..='⑳').contains(&first) {
        let value = first as u32 - '①' as u32 + 1;
        return make(Family::Circled, value, vec![], "", 1, (0, 1), false);
    }

    // 一、
    if is_cn_numeral(first) {
        let len = rest.iter().take_while(|c| is_cn_numeral(**c)).count();
        if len <= 3 && at(len) == Some('、') {
            let num: String = rest[..len].iter().collect();
            return make(
                Family::CnComma,
                parse_cn(&num)?,
                vec![],
                "、",
                len + 1,
                (0, len),
                true,
            );
        }
        return None;
    }

    // 1. / 1、 / 1） / 1.2.3
    if first.is_ascii_digit() {
        let mut parts: Vec<u32> = Vec::new();
        let mut i = 0;
        let mut last_start;
        loop {
            let s = i;
            while at(i).is_some_and(|c| c.is_ascii_digit()) {
                i += 1;
            }
            if i == s || i - s > 3 {
                return None;
            }
            last_start = s;
            parts.push(rest[s..i].iter().collect::<String>().parse().ok()?);
            if matches!(at(i), Some('.') | Some('．'))
                && at(i + 1).is_some_and(|c| c.is_ascii_digit())
            {
                i += 1;
                continue;
            }
            break;
        }
        let num_end = i;
        let next = at(i);
        if parts.len() >= 2 {
            let mut len = i;
            if matches!(next, Some('.') | Some('．')) {
                len += 1;
            }
            let after = at(len);
            let ok = match after {
                None => false,
                Some(c) if c.is_whitespace() => true,
                Some(c) if is_han(c) => is_heading || !UNIT_CHARS.contains(c),
                _ => false,
            };
            if !ok {
                return None;
            }
            let value = parts.pop().unwrap();
            let depth = parts.len() as u8 + 1;
            return make(
                Family::Dotted(depth),
                value,
                parts,
                "",
                len,
                (last_start, num_end),
                false,
            );
        }
        let value = parts[0];
        match next {
            Some(sep @ ('.' | '．' | '、')) => {
                let after = at(i + 1);
                if after.is_none_or(|c| c.is_ascii_digit() || UNIT_CHARS.contains(c) && sep != '、')
                {
                    return None;
                }
                let variant = sep.to_string();
                make(
                    Family::Arabic,
                    value,
                    vec![],
                    &variant,
                    i + 1,
                    (0, i),
                    false,
                )
            }
            Some(close @ (')' | '）')) => {
                let variant = close.to_string();
                make(
                    Family::ArabicHalf,
                    value,
                    vec![],
                    &variant,
                    i + 1,
                    (0, i),
                    false,
                )
            }
            Some(c) if c.is_whitespace() && is_heading => {
                make(Family::Dotted(1), value, vec![], "", i, (0, i), false)
            }
            _ => None,
        }
    } else {
        None
    }
}

fn render_number(p: &Prefix, value: u32) -> String {
    if p.family == Family::Circled {
        return char::from_u32('①' as u32 + value.saturating_sub(1))
            .filter(|_| (1..=20).contains(&value))
            .map_or_else(|| value.to_string(), |c| c.to_string());
    }
    if p.chinese {
        render_cn(value)
    } else {
        value.to_string()
    }
}

/// The token with its number replaced by `value`.
fn token_with(chars: &[char], p: &Prefix, value: u32) -> String {
    let mut s: String = chars[p.start..p.num_start].iter().collect();
    s.push_str(&render_number(p, value));
    s.extend(&chars[p.num_end..p.end]);
    s
}

/// The token written in `variant` (bracket width or separator).
fn token_in_variant(chars: &[char], p: &Prefix, variant: &str) -> String {
    chars[p.start..p.end]
        .iter()
        .enumerate()
        .map(|(i, &c)| {
            let i = p.start + i;
            let is_num = (p.num_start..p.num_end).contains(&i);
            match (p.family, variant) {
                _ if is_num => c,
                (Family::CnParen | Family::ArabicParen, "full") => match c {
                    '(' => '（',
                    ')' => '）',
                    c => c,
                },
                (Family::CnParen | Family::ArabicParen, "half") => match c {
                    '（' => '(',
                    '）' => ')',
                    c => c,
                },
                (Family::Arabic | Family::ArabicHalf, v) if i == p.end - 1 => {
                    v.chars().next().unwrap_or(c)
                }
                _ => c,
            }
        })
        .collect()
}

fn describe_variant(family: Family, variant: &str) -> String {
    match (family, variant) {
        (Family::CnParen | Family::ArabicParen, "full") => "全角括号".into(),
        (Family::CnParen | Family::ArabicParen, "half") => "半角括号".into(),
        (_, v) => format!("“{v}”"),
    }
}

/// Body text is anchored below every heading level.
const BODY: u8 = 99;

struct Level_ {
    family: Family,
    anchor: u8,
    last: Option<(u32, Vec<u32>, String)>,
}

fn label_is_number(label: &str) -> bool {
    label
        .chars()
        .any(|c| c.is_ascii_digit() || is_cn_numeral(c) || ('①'..='⑳').contains(&c))
}

/// Numbering findings: skipped, repeated, out-of-order or restarted
/// numbers within a heading scope, mixed styles at one level, and typed
/// numbers doubling Word's automatic numbering.
pub fn check_numbering(paras: &[ProofParagraph]) -> Vec<Issue> {
    let mut out = Vec::new();
    let mut stack: Vec<Level_> = Vec::new();
    let mut variants: HashMap<(Family, u8), (String, String)> = HashMap::new();
    for p in paras {
        if p.in_table || is_blank(&p.text) {
            continue;
        }
        let chars: Vec<char> = p.text.chars().collect();
        let prefix = parse_prefix(&p.text, p.heading_level.is_some());

        if let Some(label) = p.list_label.as_deref().filter(|l| label_is_number(l)) {
            if let Some(x) = &prefix {
                let mut end = x.end;
                while chars.get(end).is_some_and(|c| c.is_whitespace()) {
                    end += 1;
                }
                out.push(Issue::span(
                    Category::Numbering,
                    Severity::Error,
                    p.index,
                    &chars,
                    x.start,
                    end,
                    Some(String::new()),
                    format!(
                        "该段已有自动编号“{label}”，正文又手写了序号“{}”，会显示两个序号",
                        chars[x.start..x.end].iter().collect::<String>()
                    ),
                ));
            }
            if let Some(level) = p.heading_level {
                stack.retain(|l| l.anchor < level);
            }
            continue;
        }

        if let Some(level) = p.heading_level {
            let own = prefix.as_ref().map(|x| x.family);
            stack.retain(|l| l.anchor < level || l.anchor == level && Some(l.family) == own);
        }
        let Some(x) = prefix else { continue };
        let anchor = p.heading_level.unwrap_or(BODY);
        let token: String = chars[x.start..x.end].iter().collect();

        // Style consistency at this level.
        if x.variant != "mixed" && !x.variant.is_empty() {
            let key = (x.family, anchor);
            match variants.get(&key) {
                None => {
                    variants.insert(key, (x.variant.clone(), token.clone()));
                }
                Some((v, example)) if *v != x.variant => {
                    out.push(Issue::span(
                        Category::Numbering,
                        Severity::Warning,
                        p.index,
                        &chars,
                        x.start,
                        x.end,
                        Some(token_in_variant(&chars, &x, v)),
                        format!(
                            "同级序号样式不统一：前文用{}（如“{example}”），此处用{}",
                            describe_variant(x.family, v),
                            describe_variant(x.family, &x.variant)
                        ),
                    ));
                }
                Some(_) => {}
            }
        }

        let pos = stack
            .iter()
            .position(|l| l.family == x.family && l.anchor == anchor);
        let level = match pos {
            Some(i) => {
                stack.truncate(i + 1);
                &mut stack[i]
            }
            None => {
                stack.push(Level_ {
                    family: x.family,
                    anchor,
                    last: None,
                });
                stack.last_mut().unwrap()
            }
        };
        let prev = level
            .last
            .take()
            .filter(|(_, parent, _)| *parent == x.parent);
        if let Some((prev, _, prev_token)) = prev {
            let expected = prev + 1;
            let v = x.value;
            let expected_token = token_with(&chars, &x, expected);
            let issue = if v == expected {
                None
            } else if v == prev {
                Some((
                    Severity::Error,
                    None,
                    format!("序号重复：上一个同级序号也是“{prev_token}”"),
                ))
            } else if v == 1 {
                Some((
                    Severity::Warning,
                    None,
                    format!("序号在“{prev_token}”之后重新从 1 开始，但上一级标题没有变化"),
                ))
            } else if v > expected {
                Some((
                    Severity::Error,
                    Some(expected_token.clone()),
                    format!("序号跳号：上一个同级序号是“{prev_token}”，此处应为“{expected_token}”"),
                ))
            } else {
                Some((
                    Severity::Error,
                    Some(expected_token.clone()),
                    format!(
                        "序号顺序颠倒：上一个同级序号是“{prev_token}”，此处应为“{expected_token}”"
                    ),
                ))
            };
            if let Some((sev, sug, reason)) = issue {
                out.push(Issue::span(
                    Category::Numbering,
                    sev,
                    p.index,
                    &chars,
                    x.start,
                    x.end,
                    sug,
                    reason,
                ));
            }
        }
        level.last = Some((x.value, x.parent.clone(), token));
    }
    out
}

// ---------------------------------------------------------------------------
// Place names

/// Words showing a sentence mentions another place on purpose.
const CONTEXT_WORDS: &[&str] = &[
    "对比",
    "比较",
    "借鉴",
    "参照",
    "参考",
    "学习",
    "类似",
    "例如",
    "比如",
    "诸如",
    "相比",
    "相较",
    "周边",
    "毗邻",
    "相邻",
    "邻近",
    "接壤",
    "距离",
    "距",
    "往返",
    "连接",
    "通往",
    "途经",
    "沿线",
    "长三角",
    "京津冀",
    "粤港澳",
    "成渝",
    "一体化",
    "全市",
    "各区",
    "其他区",
    "兄弟",
    "等区",
    "等地",
    "等省",
    "等市",
    "等城市",
    "省市",
    "国内",
    "国外",
    "全国",
    "各地",
    "经验",
    "先进",
    "试点",
];

/// Words showing the sentence is about the project itself.
const STRONG_WORDS: &[&str] = &[
    "本项目",
    "项目位于",
    "项目地处",
    "项目选址",
    "建设地点",
    "项目所在",
    "本区",
    "我区",
    "拟建",
    "项目名称",
    "本工程",
    "工程位于",
];

/// Place-like words after a short name: 黄浦江, 长宁路, 浦东机场.
const PLACE_SUFFIXES: &[&str] = &[
    "路", "街", "道", "巷", "弄", "桥", "门", "站", "机场", "港", "码头", "江", "河", "湖", "山",
    "岛", "寺", "镇", "乡", "村", "大学", "中学", "小学", "医院", "公园", "高速", "隧道", "线",
    "号",
];

fn sentence_bounds(chars: &[char], at: usize) -> (usize, usize) {
    const END: &[char] = &['。', '！', '？', '；', '\n', '!', '?', ';'];
    let start = chars[..at]
        .iter()
        .rposition(|c| END.contains(c))
        .map_or(0, |i| i + 1);
    let end = chars[at..]
        .iter()
        .position(|c| END.contains(c))
        .map_or(chars.len(), |i| at + i);
    (start, end)
}

fn char_ranges_of(chars: &[char], needle: &str) -> Vec<(usize, usize)> {
    let needle: Vec<char> = needle.chars().collect();
    if needle.is_empty() || needle.len() > chars.len() {
        return Vec::new();
    }
    (0..=chars.len() - needle.len())
        .filter(|&i| chars[i..i + needle.len()] == needle[..])
        .map(|i| (i, i + needle.len()))
        .collect()
}

/// Char offset of `needle` in `chars`, if present.
pub(crate) fn find_chars(chars: &[char], needle: &str) -> Option<usize> {
    char_ranges_of(chars, needle).first().map(|r| r.0)
}

/// Other places at the level of the project's district, city and province
/// mentioned where the text seems to be about the project: 崇明区项目中的
/// “浦东新区”.
pub fn check_misattribution(paras: &[ProofParagraph], facts: &ProjectFacts) -> Vec<Issue> {
    let place = facts::resolve(facts);
    // name as written → (the other division, the project's own one, full name?)
    let mut candidates: HashMap<&'static str, (&'static Division, &'static Division, bool)> =
        HashMap::new();
    let mut add_level = |own: Option<&'static Division>, short_too: bool| {
        let Some(own) = own else { return };
        for other in divisions::siblings(own) {
            candidates.insert(other.name, (other, own, true));
            if short_too
                && !other.ambiguous
                && let Some(s) = other.short
            {
                candidates.entry(s).or_insert((other, own, false));
            }
        }
    };
    add_level(place.district, true);
    add_level(place.city.filter(|c| c.level == Level::City), true);
    add_level(place.province, false);
    // Never flag the project's own names.
    for own in [place.district, place.city, place.province]
        .into_iter()
        .flatten()
    {
        candidates.remove(own.name);
        if let Some(s) = own.short {
            candidates.remove(s);
        }
    }
    if candidates.is_empty() {
        return Vec::new();
    }
    let protected: Vec<&str> = [facts.name.as_deref(), facts.owner.as_deref()]
        .into_iter()
        .flatten()
        .filter(|s| s.chars().count() >= 4)
        .collect();

    let mut out = Vec::new();
    for p in paras {
        if is_blank(&p.text) {
            continue;
        }
        let chars: Vec<char> = p.text.chars().collect();
        let covered: Vec<(usize, usize)> = protected
            .iter()
            .flat_map(|n| char_ranges_of(&chars, n))
            .collect();
        for (s, e, _) in divisions::scan(&p.text) {
            let written: String = chars[s..e].iter().collect();
            let Some(&(other, own, full)) = candidates.get(written.as_str()) else {
                continue;
            };
            if covered.iter().any(|&(a, b)| a <= s && e <= b) {
                continue;
            }
            let after: String = chars[e..].iter().take(3).collect();
            if !full && PLACE_SUFFIXES.iter().any(|x| after.starts_with(x)) {
                continue;
            }
            let (ss, se) = sentence_bounds(&chars, s);
            let sentence: String = chars[ss..se].iter().collect();
            let before: String = chars[s.saturating_sub(2)..s].iter().collect();
            let like = before.ends_with('如') && !before.ends_with("例如");
            if like || CONTEXT_WORDS.iter().any(|w| sentence.contains(w)) {
                continue;
            }
            let severity = if STRONG_WORDS.iter().any(|w| sentence.contains(w)) {
                Severity::Error
            } else {
                Severity::Warning
            };
            let suggestion = if full {
                own.name.to_string()
            } else {
                own.short.unwrap_or(own.name).to_string()
            };
            out.push(Issue::span(
                Category::Misattribution,
                severity,
                p.index,
                &chars,
                s,
                e,
                Some(suggestion),
                format!(
                    "项目位于{}，此处却写了{}，疑似沿用其他项目的内容",
                    own.name, other.name
                ),
            ));
        }
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

    fn fmt(text: &str) -> Vec<(String, Option<String>, String)> {
        check_format(&paras(&[text]))
            .into_iter()
            .map(|i| (i.original, i.suggestion, i.reason))
            .collect()
    }

    fn fixed(text: &str) -> String {
        let issues = check_format(&paras(&[text]));
        let refs: Vec<&Issue> = issues.iter().collect();
        super::super::apply_issues(text, &refs).0
    }

    #[test]
    fn spaces() {
        assert_eq!(fixed("本项目 位于崇明区。"), "本项目位于崇明区。");
        assert_eq!(fixed("本项目位于崇明区 。"), "本项目位于崇明区。");
        assert_eq!(fixed("采用 BIM 技术建模。"), "采用 BIM 技术建模。");
        assert_eq!(fixed("采用  BIM技术。"), "采用 BIM技术。");
        assert_eq!(fixed("总投资   3.2亿元。"), "总投资 3.2亿元。");
        assert_eq!(fixed("本工程  建设内容如下："), "本工程建设内容如下：");
        assert_eq!(fixed("  项目概况 "), "项目概况");
        // Full-width indents and label alignment are left alone.
        assert!(fmt("　　项目概况如下。").is_empty());
        assert!(fmt("甲　　方：崇明区水务局").is_empty());
        // A space after a chapter number is conventional.
        assert!(fmt("第一章 总论").is_empty());
        assert!(fmt("1.1 项目概况").is_empty());
        assert_eq!(fixed("第一章  总论"), "第一章 总论");
        assert_eq!(fixed("一、 项目概况"), "一、项目概况");
    }

    #[test]
    fn half_width_punctuation() {
        assert_eq!(
            fixed("项目位于崇明区,总投资3.2亿元;建设期2年."),
            "项目位于崇明区，总投资3.2亿元；建设期2年."
        );
        assert_eq!(
            fixed("主要内容包括: 道路、绿化"),
            "主要内容包括：道路、绿化"
        );
        assert_eq!(fixed("是否满足要求?"), "是否满足要求？");
        assert_eq!(fixed("会议时间:10:30"), "会议时间：10:30");
        // English and numbers keep their punctuation.
        assert!(fmt("Hello, world: 1,000 m").is_empty());
        assert!(fmt("比例为1:2，容积率1.5").is_empty());
        assert_eq!(
            fixed("生态环境局(简称环境局)负责"),
            "生态环境局（简称环境局）负责"
        );
        assert!(fmt("单位为 m(米)").len() == 2);
        assert!(fmt("编号(A1)无中文").is_empty());
    }

    #[test]
    fn repeated_punctuation() {
        assert_eq!(fixed("项目已完成立项。。"), "项目已完成立项。");
        assert_eq!(fixed("道路、、绿化，，照明"), "道路、绿化，照明");
        let r = fmt("详见附件，。");
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].1, None);
        assert!(fmt("真的吗？！").is_empty());
        assert!(fmt("等等……").is_empty());
        assert!(fmt("——以上").is_empty());
    }

    #[test]
    fn brackets_and_quotes() {
        assert!(fmt("《上海市城市总体规划（2017—2035年）》").is_empty());
        let r = fmt("依据《建筑设计防火规范执行");
        assert_eq!(r.len(), 1);
        assert!(r[0].2.contains("书名号"));
        let r = fmt("详见附件）说明");
        assert!(r[0].2.contains("缺少前半个"));
        assert_eq!(
            fixed("生态环境局（简称环境局)负责"),
            "生态环境局（简称环境局）负责"
        );
        let r = fmt("所谓“海绵城市“是指");
        assert_eq!(r.len(), 1);
        assert!(r[0].2.contains("引号"));
        assert!(fmt("所谓“海绵城市”是指‘渗、滞’").is_empty());
        let r = fmt("【注意事项");
        assert!(r[0].2.contains("方括号"));
        // A label like "a)" inside a sentence is not an unbalanced bracket.
        assert!(
            fmt("包括：a)道路；b)绿化")
                .iter()
                .all(|i| !i.2.contains("缺少前半个"))
        );
        // The typed number is not checked as a bracket.
        assert!(fmt("1）项目概况").is_empty());
    }

    #[test]
    fn empty_paragraphs() {
        let mut ps = paras(&["第一段", "", "  ", "", "第二段", "", "第三段"]);
        let issues = check_format(&ps);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].paragraph, 2);
        assert!(issues[0].reason.contains("连续 3 个空段落"));
        assert_eq!(issues[0].suggestion, None);
        // Empty cells in tables are fine.
        for p in &mut ps {
            p.in_table = true;
        }
        assert!(check_format(&ps).is_empty());
        // Non-adjacent empty paragraphs (a range) are separate runs.
        let ps = vec![ProofParagraph::new(3, ""), ProofParagraph::new(7, "")];
        assert!(check_format(&ps).is_empty());
    }

    #[test]
    fn chinese_numerals() {
        for (s, n) in [
            ("一", 1),
            ("九", 9),
            ("十", 10),
            ("十一", 11),
            ("二十", 20),
            ("二十三", 23),
            ("九十九", 99),
        ] {
            assert_eq!(parse_cn(s), Some(n), "{s}");
            assert_eq!(render_cn(n), s);
        }
        assert_eq!(parse_cn("一十"), None);
        assert_eq!(parse_cn("十十"), None);
        assert_eq!(parse_cn("百"), None);
    }

    #[test]
    fn prefixes() {
        let f = |s: &str| parse_prefix(s, false).map(|p| (p.family, p.value, p.variant));
        assert_eq!(f("一、项目概况"), Some((Family::CnComma, 1, "、".into())));
        assert_eq!(f("十二、附件"), Some((Family::CnComma, 12, "、".into())));
        assert_eq!(
            f("（三）建设内容"),
            Some((Family::CnParen, 3, "full".into()))
        );
        assert_eq!(f("(三)建设内容"), Some((Family::CnParen, 3, "half".into())));
        assert_eq!(
            f("（2）道路"),
            Some((Family::ArabicParen, 2, "full".into()))
        );
        assert_eq!(f("2.道路"), Some((Family::Arabic, 2, ".".into())));
        assert_eq!(f("2、道路"), Some((Family::Arabic, 2, "、".into())));
        assert_eq!(f("2．道路"), Some((Family::Arabic, 2, "．".into())));
        assert_eq!(f("3）绿化"), Some((Family::ArabicHalf, 3, "）".into())));
        assert_eq!(f("④照明"), Some((Family::Circled, 4, "".into())));
        assert_eq!(f("第三章 总论"), Some((Family::Chapter, 3, "".into())));
        assert_eq!(f("第二部分 概述"), Some((Family::Part, 2, "".into())));
        assert_eq!(f("1.2 项目概况"), Some((Family::Dotted(2), 2, "".into())));
        let p = parse_prefix("2.3.4 排水", false).unwrap();
        assert_eq!(
            (p.family, p.value, p.parent),
            (Family::Dotted(3), 4, vec![2, 3])
        );
        // Quantities are not numbers of a list.
        assert_eq!(f("3.2亿元的投资"), None);
        assert_eq!(f("2.5万平方米"), None);
        assert_eq!(f("2024年完成"), None);
        assert_eq!(f("1.5倍"), None);
        assert_eq!(f("一是加强管理"), None);
        assert_eq!(f("第三方机构"), None);
        assert_eq!(f("1 概述"), None);
        assert_eq!(
            parse_prefix("1 概述", true).map(|p| p.family),
            Some(Family::Dotted(1))
        );
        let p = parse_prefix("  （一）概况", false).unwrap();
        assert_eq!((p.start, p.end, p.num_start, p.num_end), (2, 5, 3, 4));
    }

    fn numbering(texts: &[&str]) -> Vec<(usize, String, Option<String>, Severity)> {
        check_numbering(&paras(texts))
            .into_iter()
            .map(|i| (i.paragraph, i.original, i.suggestion, i.severity))
            .collect()
    }

    #[test]
    fn sequence_errors() {
        let r = numbering(&["一、概况", "内容", "三、建设方案", "四、投资"]);
        assert_eq!(
            r,
            [(2, "三、".into(), Some("二、".into()), Severity::Error)]
        );
        let r = numbering(&["一、概况", "二、方案", "二、投资"]);
        assert_eq!(r, [(2, "二、".into(), None, Severity::Error)]);
        let r = numbering(&["1.道路", "2.绿化", "1.照明"]);
        assert_eq!(r, [(2, "1.".into(), None, Severity::Warning)]);
        let r = numbering(&["（一）道路", "（三）绿化", "（二）照明"]);
        assert_eq!(r.len(), 2);
        assert_eq!(r[1].2.as_deref(), Some("（四）"));
        let r = numbering(&["①道路", "③绿化"]);
        assert_eq!(r[0].2.as_deref(), Some("②"));
        let r = numbering(&["第一章 总论", "第三章 方案"]);
        assert_eq!(r[0].2.as_deref(), Some("第二章"));
    }

    #[test]
    fn nested_levels_restart_under_a_new_parent() {
        let ok = [
            "一、项目概况",
            "（一）项目名称",
            "1.道路",
            "2.绿化",
            "（二）建设地点",
            "1.位置",
            "2.范围",
            "二、建设方案",
            "（一）总体方案",
            "1.思路",
            "①原则",
            "②目标",
            "2.布局",
            "①分区",
        ];
        assert!(numbering(&ok).is_empty(), "{:?}", numbering(&ok));
        let r = numbering(&["1.1 概况", "1.2 背景", "2.1 方案", "2.3 布局"]);
        assert_eq!(r, [(3, "2.3".into(), Some("2.2".into()), Severity::Error)]);
    }

    #[test]
    fn headings_reset_lists() {
        let ps = vec![
            ProofParagraph::heading(0, "项目概况", 0),
            ProofParagraph::new(1, "1.道路"),
            ProofParagraph::new(2, "2.绿化"),
            ProofParagraph::heading(3, "建设方案", 0),
            ProofParagraph::new(4, "1.思路"),
            ProofParagraph::heading(5, "1.1 投资", 1),
            ProofParagraph::heading(6, "1.2 资金", 1),
            ProofParagraph::new(7, "1.来源"),
        ];
        assert!(
            check_numbering(&ps).is_empty(),
            "{:?}",
            check_numbering(&ps)
        );
        // A heading level's own numbers continue across its siblings.
        let ps = vec![
            ProofParagraph::heading(0, "一、概况", 0),
            ProofParagraph::new(1, "1.道路"),
            ProofParagraph::heading(2, "三、方案", 0),
        ];
        let r = check_numbering(&ps);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].suggestion.as_deref(), Some("二、"));
    }

    #[test]
    fn mixed_styles() {
        let r = numbering(&["（一）道路", "(二)绿化"]);
        assert_eq!(
            r,
            [(1, "(二)".into(), Some("（二）".into()), Severity::Warning)]
        );
        let r = numbering(&["1.道路", "2、绿化"]);
        assert_eq!(r, [(1, "2、".into(), Some("2.".into()), Severity::Warning)]);
        let r = numbering(&["(1)道路", "（2）绿化"]);
        assert_eq!(r[0].2.as_deref(), Some("(2)"));
    }

    #[test]
    fn auto_numbering_doubled() {
        let mut p = ProofParagraph::new(0, "1. 项目概况");
        p.list_label = Some("1.".into());
        let r = check_numbering(&[p.clone()]);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].original, "1. ");
        assert_eq!(r[0].suggestion.as_deref(), Some(""));
        // Bullets are not numbers.
        p.list_label = Some("•".into());
        p.text = "1.项目概况".into();
        let ps = vec![p.clone(), ProofParagraph::new(1, "2.道路")];
        assert!(check_numbering(&ps).is_empty());
        // Auto-numbered paragraphs do not join typed sequences.
        let mut auto = ProofParagraph::new(1, "绿化");
        auto.list_label = Some("2.".into());
        let ps = vec![
            ProofParagraph::new(0, "1.道路"),
            auto,
            ProofParagraph::new(2, "2.照明"),
        ];
        assert!(check_numbering(&ps).is_empty());
    }

    fn chongming() -> ProjectFacts {
        ProjectFacts {
            name: Some("崇明区城桥镇污水管网改造工程".into()),
            district: Some("崇明区".into()),
            ..Default::default()
        }
    }

    fn misattributed(texts: &[&str]) -> Vec<(usize, String, Option<String>, Severity)> {
        check_misattribution(&paras(texts), &chongming())
            .into_iter()
            .map(|i| (i.paragraph, i.original, i.suggestion, i.severity))
            .collect()
    }

    #[test]
    fn flags_other_districts() {
        let r = misattributed(&[
            "本项目位于上海市浦东新区城桥镇。",
            "项目建成后将改善浦东地区的水环境。",
            "崇明区水务局负责实施。",
        ]);
        assert_eq!(
            r,
            [
                (0, "浦东新区".into(), Some("崇明区".into()), Severity::Error),
                (1, "浦东".into(), Some("崇明".into()), Severity::Warning),
            ]
        );
    }

    #[test]
    fn allows_comparisons_and_place_words() {
        let r = misattributed(&[
            "借鉴浦东新区的成功经验，推进管网改造。",
            "与闵行区相比，崇明区管网密度较低。",
            "项目距浦东国际机场约80公里。",
            "黄浦江上游来水水质较好。",
            "如嘉定区已建成类似设施。",
            "施工单位位于长宁路100号。",
            "坚持绿水青山就是金山银山。",
            "崇明区城桥镇污水管网改造工程由崇明区负责。",
        ]);
        assert!(r.is_empty(), "{r:?}");
    }

    #[test]
    fn city_and_province_levels() {
        let facts = ProjectFacts {
            city: Some("苏州市".into()),
            ..Default::default()
        };
        let ps = paras(&["本项目位于无锡市工业园区。", "浙江省", "苏州工业园区"]);
        let r: Vec<_> = check_misattribution(&ps, &facts)
            .into_iter()
            .map(|i| (i.original, i.suggestion))
            .collect();
        assert_eq!(
            r,
            [
                ("无锡市".into(), Some("苏州市".into())),
                ("浙江省".into(), Some("江苏省".into()))
            ]
        );
        // Unknown places leave the rule silent.
        let facts = ProjectFacts {
            district: Some("不存在区".into()),
            ..Default::default()
        };
        assert!(check_misattribution(&ps, &facts).is_empty());
    }

    #[test]
    fn project_name_is_protected() {
        let facts = ProjectFacts {
            name: Some("浦东至崇明水利连通工程".into()),
            district: Some("崇明区".into()),
            ..Default::default()
        };
        let ps = paras(&["浦东至崇明水利连通工程已完成设计。"]);
        assert!(check_misattribution(&ps, &facts).is_empty());
    }
}
