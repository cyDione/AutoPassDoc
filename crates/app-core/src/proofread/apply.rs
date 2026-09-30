//! Writing accepted proofreading issues into the document, and the web
//! lookup the citation check uses.

use std::collections::BTreeMap;

use docx_engine::{Document, EditOptions};
use futures::FutureExt;
use futures::future::BoxFuture;

use super::model::{CitationLookup, SearchSnippet};
use super::{Citation, Issue, apply_issues};
use crate::core::Core;
use crate::error::{Error, Result};
use crate::web::WebMode;

/// Label of the undo step.
pub const PROOFREAD_LABEL: &str = "文档校对";

impl Core {
    /// Applies `issues` as one undo step, as tracked changes or direct edits
    /// per the fix settings. Issues whose text changed since the check, or
    /// that have no suggestion, are skipped. Returns the ids applied.
    pub fn apply_proof_issues(&self, doc: &mut Document, issues: &[Issue]) -> Result<Vec<String>> {
        let mut by_paragraph: BTreeMap<usize, Vec<&Issue>> = BTreeMap::new();
        for issue in issues.iter().filter(|i| i.suggestion.is_some()) {
            if issue.paragraph < doc.paragraphs.len() {
                by_paragraph.entry(issue.paragraph).or_default().push(issue);
            }
        }
        let mut changes = Vec::new();
        let mut applied = Vec::new();
        for (index, list) in by_paragraph {
            let text = doc.editable_text(index);
            let (new, ids) = apply_issues(&text, &list);
            if !ids.is_empty() && new != text {
                changes.push((index, new));
                applied.extend(ids);
            }
        }
        if changes.is_empty() {
            return Err(Error::Invalid(
                "没有可以应用的修改：原文已变化，请重新校对".into(),
            ));
        }
        let fix = self.settings()?.fix;
        let options = EditOptions::new(fix.edit_mode, fix.author.clone());
        doc.group(PROOFREAD_LABEL, |doc| {
            doc.replace_paragraphs(&changes, &options, PROOFREAD_LABEL)
        })?;
        Ok(applied)
    }

    /// The citation lookup to pass to [`Core::proofread`]: `None` when web
    /// access is switched off.
    pub fn citation_lookup(&self) -> Result<Option<&dyn CitationLookup>> {
        Ok((self.settings()?.web.mode != WebMode::Off).then_some(self as &dyn CitationLookup))
    }
}

/// Searches “标题 + 废止 / 修订 / 最新版” with the configured web search.
impl CitationLookup for Core {
    fn lookup<'a>(&'a self, citation: &'a Citation) -> BoxFuture<'a, Result<Vec<SearchSnippet>>> {
        async move {
            let number = citation
                .standard_no
                .as_deref()
                .or(citation.doc_no.as_deref())
                .unwrap_or_default();
            let query = format!("《{}》 {number} 废止 修订 最新版", citation.title);
            let outcome = self.web_search(&query).await?;
            // Search engine results off the whitelist are not authoritative.
            let local = outcome.via == "local";
            Ok(outcome
                .results
                .into_iter()
                .filter(|r| r.trusted || !local)
                .map(|r| SearchSnippet {
                    title: r.title,
                    url: r.url,
                    snippet: r.snippet,
                })
                .collect())
        }
        .boxed()
    }
}
