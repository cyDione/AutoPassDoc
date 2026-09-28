//! Tokenizing for the FTS5 index: jieba search-mode words separated by spaces,
//! plus canonical tokens for 文号 and ordinal references (第X条), so exact
//! lookups hit regardless of bracket style or numeral form.

use std::collections::HashSet;
use std::sync::LazyLock;

use jieba_rs::Jieba;
use regex::Regex;

use crate::metadata::find_doc_numbers;
use crate::text::{normalize, parse_number};

static JIEBA: LazyLock<Jieba> = LazyLock::new(Jieba::new);

static ORDINAL_REF: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new("第([0-9一二三四五六七八九十百千零〇○两]+)(条|章|节|编|款|项|部分)").unwrap()
});

/// Upper bound on distinct query terms; long queries (a whole paragraph)
/// keep their first terms.
const MAX_QUERY_TERMS: usize = 128;

static STOPWORDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        "的", "了", "和", "与", "及", "或", "等", "在", "是", "为", "对", "由", "将", "把", "被",
        "于", "之", "其", "以", "而", "并", "且", "也", "都", "就", "这", "那", "此", "该", "各",
        "有", "个", "第", "条", "号", "款", "项", "着", "从", "向", "到", "按", "如", "若", "则",
        "但", "又", "还", "所", "已", "我", "你", "他", "她", "它", "们", "我们", "你们", "他们",
        "吗", "呢", "吧", "啊", "及其", "以及", "或者", "并且", "关于", "对于", "一个", "这个",
        "那个", "这些", "那些", "什么", "怎么", "如何", "哪些", "the", "a", "an", "of", "and",
        "or", "to", "in", "on", "for", "is", "are", "be", "with", "by", "as", "at",
    ]
    .into_iter()
    .collect()
});

/// Loads the jieba dictionary; the first tokenization pays for it otherwise.
pub(crate) fn warm_up() {
    LazyLock::force(&JIEBA);
}

fn keep(word: &str) -> bool {
    word.chars().any(char::is_alphanumeric) && !STOPWORDS.contains(word)
}

/// Canonical token of a normalised 文号: `国办发〔2024〕12号` → `国办发202412号`.
pub(crate) fn doc_number_token(normalized: &str) -> String {
    normalized.chars().filter(|c| c.is_alphanumeric()).collect()
}

/// Special tokens of `norm` and their byte spans.
fn special_tokens(norm: &str) -> Vec<(String, (usize, usize))> {
    let mut out: Vec<(String, (usize, usize))> = find_doc_numbers(norm)
        .into_iter()
        .map(|m| (doc_number_token(&m.normalized), (m.start, m.end)))
        .collect();
    for c in ORDINAL_REF.captures_iter(norm) {
        if let Some(n) = parse_number(&c[1]) {
            let m = c.get(0).unwrap();
            out.push((format!("第{n}{}", &c[2]), (m.start(), m.end())));
        }
    }
    out
}

/// Space-separated tokens to store in the FTS index.
pub(crate) fn index_text(text: &str) -> String {
    let norm = normalize(text).to_lowercase();
    let mut out: Vec<String> = special_tokens(&norm).into_iter().map(|t| t.0).collect();
    out.extend(
        JIEBA
            .cut_for_search(&norm, true)
            .into_iter()
            .filter(|t| keep(t.word))
            .map(|t| t.word.to_string()),
    );
    out.join(" ")
}

/// Distinct query terms. Text covered by a special token is not segmented
/// again, so "第二十条" does not also match every "二十".
pub(crate) fn query_terms(text: &str) -> Vec<String> {
    let norm = normalize(text).to_lowercase();
    let special = special_tokens(&norm);
    let mut rest = norm.clone();
    for (_, (s, e)) in &special {
        // Spans may overlap; blank each byte range without splitting chars.
        let s = (0..=*s)
            .rev()
            .find(|&i| rest.is_char_boundary(i))
            .unwrap_or(0);
        let e = (*e..=rest.len())
            .find(|&i| rest.is_char_boundary(i))
            .unwrap_or(rest.len());
        rest.replace_range(s..e, &" ".repeat(e - s));
    }
    let mut seen = HashSet::new();
    special
        .into_iter()
        .map(|t| t.0)
        .chain(
            JIEBA
                .cut_for_search(&rest, true)
                .into_iter()
                .filter(|t| keep(t.word))
                .map(|t| t.word.to_string()),
        )
        .filter(|t| seen.insert(t.clone()))
        .take(MAX_QUERY_TERMS)
        .collect()
}

/// FTS5 MATCH expression: every term quoted, joined with OR.
pub(crate) fn match_expression(terms: &[String]) -> String {
    terms
        .iter()
        .map(|t| format!("\"{}\"", t.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" OR ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn special_tokens_are_canonical() {
        let q = query_terms("国办发[2024]12号 第二十条");
        assert!(q.contains(&"国办发202412号".to_string()), "{q:?}");
        assert!(q.contains(&"第20条".to_string()), "{q:?}");
        assert!(!q.iter().any(|t| t == "二十" || t == "2024"), "{q:?}");
        let indexed = index_text("依照本法第20条和国办发〔2024〕12号的规定");
        assert!(indexed.split(' ').any(|t| t == "第20条"), "{indexed}");
        assert!(
            indexed.split(' ').any(|t| t == "国办发202412号"),
            "{indexed}"
        );
    }

    #[test]
    fn drops_stopwords_and_punctuation() {
        let q = query_terms("关于数据安全的规定，。！");
        assert!(
            q.iter().all(|t| t != "的" && t != "，" && t != "关于"),
            "{q:?}"
        );
        assert!(q.contains(&"数据".to_string()), "{q:?}");
    }
}
