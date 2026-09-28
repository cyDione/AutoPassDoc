//! Mapping comment authors to reviewers.
//!
//! Reviewers often sign comments with a nickname or a machine default such
//! as "Administrator". A user-made mapping from a signature to a reviewer is
//! remembered; mappings for generic signatures apply only to the document
//! they were made in, so different people who all appear as "Administrator"
//! are not merged.

use std::collections::HashMap;

use serde::Serialize;

use crate::error::Result;
use crate::store::{Reviewer, Store};

const GENERIC: &[&str] = &[
    "administrator",
    "admin",
    "user",
    "author",
    "unknown",
    "microsoft office user",
    "microsoft office 用户",
    "windows 用户",
    "windows user",
    "作者",
    "用户",
    "未知",
    "微软用户",
    "lenovo",
    "dell",
    "hp",
    "thinkpad",
    "pc",
];

/// Whether an author signature is a machine default shared by many people.
pub fn is_generic(author: &str) -> bool {
    let a = author.trim().to_lowercase();
    a.is_empty() || GENERIC.contains(&a.as_str())
}

/// The reviewer an author signature maps to in the given document.
pub fn resolve(
    store: &Store,
    author: &str,
    initials: &str,
    doc_key: &str,
) -> Result<Option<Reviewer>> {
    Ok(resolve_all(store, doc_key)?
        .get(&(author.to_string(), initials.to_string()))
        .cloned())
}

/// All signature → reviewer mappings that apply in a document. A mapping
/// with matching initials wins over one made without initials.
pub fn resolve_all(store: &Store, doc_key: &str) -> Result<HashMap<(String, String), Reviewer>> {
    let reviewers: HashMap<i64, Reviewer> =
        store.reviewers()?.into_iter().map(|r| (r.id, r)).collect();
    let mut out = HashMap::new();
    let mut aliases = store.aliases()?;
    // Global first, so document-specific mappings override them.
    aliases.sort_by_key(|a| !a.doc_key.is_empty());
    for alias in aliases {
        if !alias.doc_key.is_empty() && alias.doc_key != doc_key {
            continue;
        }
        if alias.doc_key.is_empty() && is_generic(&alias.author) {
            continue;
        }
        if let Some(r) = reviewers.get(&alias.reviewer_id) {
            out.insert((alias.author, alias.initials), r.clone());
        }
    }
    Ok(out)
}

/// Looks up a signature, falling back to a mapping made without initials,
/// then to a reviewer whose name equals the signature.
pub fn lookup(
    map: &HashMap<(String, String), Reviewer>,
    reviewers: &[Reviewer],
    author: &str,
    initials: &str,
) -> Option<Reviewer> {
    map.get(&(author.to_string(), initials.to_string()))
        .or_else(|| map.get(&(author.to_string(), String::new())))
        .cloned()
        .or_else(|| {
            (!is_generic(author))
                .then(|| reviewers.iter().find(|r| r.name == author.trim()).cloned())
                .flatten()
        })
}

/// Records that `author` is `reviewer_id`. Generic signatures are recorded
/// for this document only.
pub fn assign(
    store: &Store,
    author: &str,
    initials: &str,
    doc_key: &str,
    reviewer_id: Option<i64>,
) -> Result<()> {
    let scope = if is_generic(author) { doc_key } else { "" };
    store.set_alias(author, initials, scope, reviewer_id)?;
    if !initials.is_empty() {
        // Also cover the same signature without initials.
        store.set_alias(author, "", scope, reviewer_id)?;
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthorView {
    pub author: String,
    pub initials: String,
    pub comment_count: usize,
    pub reviewer: Option<Reviewer>,
    pub generic: bool,
}

/// Comment authors of a document with the reviewer each maps to.
pub fn authors(
    store: &Store,
    doc: &docx_engine::Document,
    doc_key: &str,
) -> Result<Vec<AuthorView>> {
    let map = resolve_all(store, doc_key)?;
    let reviewers = store.reviewers()?;
    let mut counts: Vec<((String, String), usize)> = Vec::new();
    for c in doc.comments.iter().filter(|c| c.parent_id.is_none()) {
        let key = (c.author.clone(), c.initials.clone().unwrap_or_default());
        match counts.iter_mut().find(|(k, _)| *k == key) {
            Some((_, n)) => *n += 1,
            None => counts.push((key, 1)),
        }
    }
    Ok(counts
        .into_iter()
        .map(|((author, initials), comment_count)| AuthorView {
            reviewer: lookup(&map, &reviewers, &author, &initials),
            generic: is_generic(&author),
            author,
            initials,
            comment_count,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generic_signatures_stay_per_document() {
        let store = Store::open_in_memory().unwrap();
        let zhang = store.create_reviewer("张处长", "").unwrap();
        let li = store.create_reviewer("李教授", "").unwrap();

        assign(&store, "Administrator", "A", "doc1", Some(zhang.id)).unwrap();
        assign(&store, "lq", "", "doc1", Some(li.id)).unwrap();

        let reviewers = store.reviewers().unwrap();
        let in_doc1 = resolve_all(&store, "doc1").unwrap();
        let in_doc2 = resolve_all(&store, "doc2").unwrap();
        assert_eq!(
            lookup(&in_doc1, &reviewers, "Administrator", "A")
                .unwrap()
                .name,
            "张处长"
        );
        assert_eq!(
            lookup(&in_doc1, &reviewers, "Administrator", "")
                .unwrap()
                .name,
            "张处长"
        );
        assert!(lookup(&in_doc2, &reviewers, "Administrator", "A").is_none());
        assert_eq!(
            lookup(&in_doc2, &reviewers, "lq", "").unwrap().name,
            "李教授",
            "real nicknames are global"
        );
        assert_eq!(
            lookup(&in_doc2, &reviewers, "李教授", "L").unwrap().name,
            "李教授",
            "exact names match"
        );

        assign(&store, "lq", "", "doc2", None).unwrap();
        assert!(resolve(&store, "lq", "", "doc2").unwrap().is_none());
    }
}
