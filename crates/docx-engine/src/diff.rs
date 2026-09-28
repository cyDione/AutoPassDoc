//! Token diff shared by the AI-fix preview and the write-back.
//!
//! Chinese is compared character by character; a run of Latin letters or
//! digits is one token, so "manage" → "management" reads as one replacement.
//! Placeholders such as `⟦图⟧` (see [`crate::edit`]) are single tokens.

use std::ops::Range;

use serde::Serialize;
use similar::{Algorithm, DiffOp, capture_diff_slices};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DiffKind {
    Equal,
    Insert,
    Delete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiffSpan {
    pub kind: DiffKind,
    pub text: String,
}

pub fn tokenize(s: &str) -> Vec<&str> {
    let mut tokens = Vec::new();
    let mut rest = s;
    while let Some(c) = rest.chars().next() {
        let len = if c == '⟦' {
            rest.find('⟧')
                .filter(|&end| end <= 32)
                .map_or(c.len_utf8(), |end| end + '⟧'.len_utf8())
        } else if c.is_ascii_alphanumeric() {
            rest.find(|c: char| !c.is_ascii_alphanumeric())
                .unwrap_or(rest.len())
        } else {
            c.len_utf8()
        };
        tokens.push(&rest[..len]);
        rest = &rest[len..];
    }
    tokens
}

/// A change: tokens `old` are replaced by tokens `new`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Hunk {
    pub old: Range<usize>,
    pub new: Range<usize>,
}

pub(crate) fn hunks(old: &[&str], new: &[&str]) -> Vec<Hunk> {
    let mut hunks: Vec<Hunk> = Vec::new();
    for op in capture_diff_slices(Algorithm::Myers, old, new) {
        let (o, n) = match op {
            DiffOp::Equal { .. } => continue,
            DiffOp::Delete {
                old_index,
                old_len,
                new_index,
            } => (old_index..old_index + old_len, new_index..new_index),
            DiffOp::Insert {
                old_index,
                new_index,
                new_len,
            } => (old_index..old_index, new_index..new_index + new_len),
            DiffOp::Replace {
                old_index,
                old_len,
                new_index,
                new_len,
            } => (
                old_index..old_index + old_len,
                new_index..new_index + new_len,
            ),
        };
        match hunks.last_mut() {
            Some(last) if last.old.end == o.start && last.new.end == n.start => {
                last.old.end = o.end;
                last.new.end = n.end;
            }
            _ => hunks.push(Hunk { old: o, new: n }),
        }
    }
    merge_short_equalities(old, new, hunks)
}

/// Absorbs an unchanged stretch of one or two characters into the
/// surrounding changes when it is no longer than the changes on either side,
/// so "加强管理" → "加强监管" reads as a word replacement rather than a
/// scatter of single characters.
fn merge_short_equalities(old: &[&str], new: &[&str], hunks: Vec<Hunk>) -> Vec<Hunk> {
    let chars = |tokens: &[&str]| tokens.iter().map(|t| t.chars().count()).sum::<usize>();
    let size = |h: &Hunk| chars(&old[h.old.clone()]).max(chars(&new[h.new.clone()]));
    let mut merged: Vec<Hunk> = Vec::with_capacity(hunks.len());
    for h in hunks {
        if let Some(last) = merged.last_mut() {
            let gap = chars(&old[last.old.end..h.old.start]);
            if gap <= 2 && gap <= size(last) && gap <= size(&h) {
                last.old.end = h.old.end;
                last.new.end = h.new.end;
                continue;
            }
        }
        merged.push(h);
    }
    merged
}

/// Diff of two texts as display spans.
pub fn diff(old: &str, new: &str) -> Vec<DiffSpan> {
    let (a, b) = (tokenize(old), tokenize(new));
    let mut spans: Vec<DiffSpan> = Vec::new();
    let mut push = |kind: DiffKind, tokens: &[&str]| {
        if tokens.is_empty() {
            return;
        }
        match spans.last_mut() {
            Some(last) if last.kind == kind => last.text.extend(tokens.iter().copied()),
            _ => spans.push(DiffSpan {
                kind,
                text: tokens.concat(),
            }),
        }
    };
    let mut at = 0;
    for h in hunks(&a, &b) {
        push(DiffKind::Equal, &a[at..h.old.start]);
        push(DiffKind::Delete, &a[h.old.clone()]);
        push(DiffKind::Insert, &b[h.new.clone()]);
        at = h.old.end;
    }
    push(DiffKind::Equal, &a[at..]);
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(spans: &[DiffSpan]) -> String {
        spans
            .iter()
            .map(|s| match s.kind {
                DiffKind::Equal => s.text.clone(),
                DiffKind::Delete => format!("[-{}]", s.text),
                DiffKind::Insert => format!("[+{}]", s.text),
            })
            .collect()
    }

    #[test]
    fn tokens() {
        assert_eq!(
            tokenize("共计2024年ABC项目⟦图⟧。"),
            ["共", "计", "2024", "年", "ABC", "项", "目", "⟦图⟧", "。"]
        );
        assert_eq!(tokenize("⟦未闭合"), ["⟦", "未", "闭", "合"]);
    }

    #[test]
    fn words_and_numbers_change_as_a_whole() {
        assert_eq!(
            render(&diff("投资2023万元 manage", "投资2024万元 management")),
            "投资[-2023][+2024]万元 [-manage][+management]"
        );
    }

    #[test]
    fn short_equalities_are_absorbed() {
        assert_eq!(
            render(&diff("要进一步加强管理", "要切实加强监管")),
            "要[-进一步][+切实]加强[-管理][+监管]"
        );
        assert_eq!(render(&diff("相同", "相同")), "相同");
        assert_eq!(render(&diff("", "新增")), "[+新增]");
    }
}
