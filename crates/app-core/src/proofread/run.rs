//! Running a proofread: rules, then the model section by section (cached per
//! paragraph), then the cross-document figure comparison and the citation
//! status checks, merged into one report.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};

use futures::StreamExt;
use models::{ChatRequest, Message, ThinkingLevel};
use serde::{Deserialize, Serialize};

use super::model::{self, CitationLookup, CitationStatus, Fact};
use super::{
    Category, Citation, CitationKind, Issue, ProjectFacts, ProofInput, ProofParagraph, check_rules,
    extract_citations, extract_facts, hash, merge_issues,
};
use crate::core::{Core, RoleName, Target};
use crate::error::{Error, Result};
use crate::fix::prompt::estimate_tokens;
use crate::store::now;

/// Most characters in one model section.
pub const SECTION_CHARS: usize = 3000;
/// Most conflicting-figure groups sent for confirmation.
const MAX_GROUPS: usize = 30;
/// Most citations looked up in one run.
const MAX_CITATIONS: usize = 60;
/// Section results are reused for this long.
const CACHE_DAYS: i64 = 90;
/// Citation statuses are reused for this long; documents get repealed.
const CITATION_CACHE_DAYS: i64 = 7;
/// Bump when prompts or the cached shape change.
const CACHE_VERSION: &str = "v2";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProofOptions {
    /// Identifies the document for the result cache (e.g. its path).
    pub doc_key: String,
    /// Checks to run; empty = all.
    pub categories: Vec<Category>,
    /// Ask the chat model (typos, contradictions, citation status). Without
    /// it only the rules run.
    pub use_model: bool,
    /// Project facts confirmed by the user; read from the input when
    /// `None`. When proofreading a range, pass the facts of the whole
    /// document.
    pub facts: Option<ProjectFacts>,
}

impl Default for ProofOptions {
    fn default() -> Self {
        Self {
            doc_key: String::new(),
            categories: Category::ALL.to_vec(),
            use_model: true,
            facts: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProofStage {
    Rules,
    Model,
    Consistency,
    Citations,
    Done,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProofProgress {
    pub stage: ProofStage,
    pub done: usize,
    pub total: usize,
    /// Findings of the step that just finished, so they can be shown while
    /// the rest runs. The final report supersedes them (merged, sorted).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub found: Vec<Issue>,
}

impl ProofProgress {
    fn step(stage: ProofStage, done: usize, total: usize) -> Self {
        Self {
            stage,
            done,
            total,
            found: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CitationReport {
    #[serde(flatten)]
    pub citation: Citation,
    /// `None` when it was not checked (no web search, or not a law,
    /// standard or policy).
    pub status: Option<CitationStatus>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProofReport {
    pub facts: ProjectFacts,
    /// Sorted by paragraph, then position.
    pub issues: Vec<Issue>,
    pub citations: Vec<CitationReport>,
    /// Issues per category (`typo`, `format`, …).
    pub counts: BTreeMap<String, usize>,
    /// Model sections in this run, and how many came from the cache.
    pub sections: usize,
    pub cached_sections: usize,
    pub model_calls: usize,
    /// Steps that failed (the rest of the report is still valid).
    pub failures: Vec<String>,
    /// Stopped by the user before the end.
    pub cancelled: bool,
}

/// One paragraph's model findings as cached: offsets are in its text, so
/// they stay valid wherever the paragraph moves.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Cached {
    issues: Vec<Issue>,
    facts: Vec<Fact>,
}

fn para_hash(p: &ProofParagraph) -> String {
    hash(&[&p.text])
}

fn range_label(paras: &[&ProofParagraph]) -> String {
    match (paras.first(), paras.last()) {
        (Some(a), Some(b)) if a.index != b.index => format!("第{}–{}段", a.index, b.index),
        (Some(a), _) => format!("第{}段", a.index),
        _ => String::new(),
    }
}

impl Core {
    async fn proof_ask(&self, chat: &Target, system: &str, user: String) -> Result<String> {
        let mut req = ChatRequest::new(
            chat.model.clone(),
            vec![Message::system(system), Message::user(user)],
        );
        req.profile = chat.profile.clone();
        req.thinking = chat.thinking;
        if !self.settings()?.proofread.thinking {
            req.thinking = Some(ThinkingLevel::Off);
        }
        req.json_output = true;
        req.temperature = Some(0.1);
        req.max_tokens = Some(chat.profile.max_output_tokens.clamp(1024, 8192));
        let response = self
            .client()
            .chat(&chat.provider, &req)
            .await
            .map_err(|e| Error::Invalid(format!("大语言模型调用失败：{e}")))?;
        Ok(response.content)
    }

    /// Proofreads `input`: rules first, then (with `options.use_model`) the
    /// chat model section by section, the comparison of figures across the
    /// document and, when `lookup` is given, the status of each cited
    /// document. `progress` reports each step; setting `cancel` stops
    /// before the next model request and returns what is done.
    pub async fn proofread(
        &self,
        input: &ProofInput,
        options: &ProofOptions,
        lookup: Option<&dyn CitationLookup>,
        progress: impl Fn(ProofProgress) + Send + Sync,
        cancel: &AtomicBool,
    ) -> Result<ProofReport> {
        let paras = &input.paragraphs;
        let categories: Vec<Category> = if options.categories.is_empty() {
            Category::ALL.to_vec()
        } else {
            options.categories.clone()
        };
        let wants = |c: Category| categories.contains(&c);
        let facts = options
            .facts
            .clone()
            .unwrap_or_else(|| extract_facts(paras));

        progress(ProofProgress::step(ProofStage::Rules, 0, 1));
        let mut issues = check_rules(input, &facts, &categories);
        progress(ProofProgress {
            found: issues
                .iter()
                .filter(|i| wants(i.category))
                .cloned()
                .collect(),
            ..ProofProgress::step(ProofStage::Rules, 1, 1)
        });
        let citations = extract_citations(paras);
        let mut report = ProofReport {
            facts: facts.clone(),
            citations: citations
                .iter()
                .cloned()
                .map(|citation| CitationReport {
                    citation,
                    status: None,
                })
                .collect(),
            ..Default::default()
        };

        let model_checks: Vec<Category> = [
            Category::Typo,
            Category::Misattribution,
            Category::Consistency,
        ]
        .into_iter()
        .filter(|c| wants(*c))
        .collect();
        let check_citations = wants(Category::Citation) && lookup.is_some();
        if options.use_model && (!model_checks.is_empty() || check_citations) {
            let chat = self.require(RoleName::Chat)?;
            let concurrency = self.settings()?.proofread.concurrency.max(1);
            if !model_checks.is_empty() {
                let facts_found = self
                    .proof_sections(
                        paras,
                        &facts,
                        &model_checks,
                        options,
                        &chat,
                        concurrency,
                        &mut issues,
                        &mut report,
                        &progress,
                        cancel,
                    )
                    .await?;
                if wants(Category::Consistency) && !cancel.load(Ordering::Relaxed) {
                    self.proof_consistency(
                        paras,
                        &facts_found,
                        options,
                        &chat,
                        &mut issues,
                        &mut report,
                        &progress,
                    )
                    .await?;
                }
            }
            if check_citations
                && !cancel.load(Ordering::Relaxed)
                && let Some(lookup) = lookup
            {
                self.proof_citations(
                    paras,
                    lookup,
                    &chat,
                    concurrency,
                    &mut issues,
                    &mut report,
                    &progress,
                    cancel,
                )
                .await?;
            }
        }

        let issues: Vec<Issue> = issues.into_iter().filter(|i| wants(i.category)).collect();
        report.issues = merge_issues(issues);
        for c in Category::ALL {
            let n = report.issues.iter().filter(|i| i.category == c).count();
            if n > 0 {
                report.counts.insert(c.as_str().to_string(), n);
            }
        }
        report.cancelled = cancel.load(Ordering::Relaxed);
        progress(ProofProgress::step(ProofStage::Done, 1, 1));
        Ok(report)
    }

    /// The section checks; returns every figure found (cached or new).
    #[allow(clippy::too_many_arguments)]
    async fn proof_sections(
        &self,
        paras: &[ProofParagraph],
        facts: &ProjectFacts,
        checks: &[Category],
        options: &ProofOptions,
        chat: &Target,
        concurrency: usize,
        issues: &mut Vec<Issue>,
        report: &mut ProofReport,
        progress: &(impl Fn(ProofProgress) + Send + Sync),
        cancel: &AtomicBool,
    ) -> Result<Vec<Fact>> {
        let want_facts = checks.contains(&Category::Consistency);
        let head = model::check_prompt(facts, &[], want_facts);
        let budget = (chat.profile.context_window as usize * 6 / 10)
            .saturating_sub(
                estimate_tokens(model::CHECK_SYSTEM)
                    + estimate_tokens(&head)
                    + chat.profile.max_output_tokens.clamp(1024, 8192) as usize,
            )
            .clamp(500, SECTION_CHARS);
        let sections = model::segment(paras, budget);
        let checks_key = format!(
            "{CACHE_VERSION}|{}|{}",
            checks
                .iter()
                .map(|c| c.as_str())
                .collect::<Vec<_>>()
                .join(","),
            hash(&[&facts.prompt_text()])
        );
        let hashes: Vec<String> = sections
            .iter()
            .flatten()
            .map(|&i| para_hash(&paras[i]))
            .collect();
        let cached = self.store().proofread_cached(
            &options.doc_key,
            &hashes,
            &checks_key,
            now() - CACHE_DAYS * 86_400,
        )?;

        let mut found_facts = Vec::new();
        let mut found_issues = Vec::new();
        let mut todo = Vec::new();
        let total = sections.len();
        report.sections = total;
        for section in sections {
            let all_cached = section
                .iter()
                .all(|&i| cached.contains_key(&para_hash(&paras[i])));
            if !all_cached {
                todo.push(section);
                continue;
            }
            report.cached_sections += 1;
            for &i in &section {
                let p = &paras[i];
                let Ok(c) = serde_json::from_str::<Cached>(&cached[&para_hash(p)]) else {
                    continue;
                };
                found_issues.extend(c.issues.into_iter().map(|mut x| {
                    x.paragraph = p.index;
                    x
                }));
                found_facts.extend(c.facts.into_iter().map(|mut f| {
                    f.paragraph = p.index;
                    f
                }));
            }
        }
        let mut done = report.cached_sections;
        let shown = |found: &[Issue]| -> Vec<Issue> {
            found
                .iter()
                .filter(|i| checks.contains(&i.category))
                .cloned()
                .collect()
        };
        progress(ProofProgress {
            found: shown(&found_issues),
            ..ProofProgress::step(ProofStage::Model, done, total)
        });
        issues.append(&mut found_issues);

        let mut stream = futures::stream::iter(todo)
            .map(|section| async move {
                let members: Vec<&ProofParagraph> = section.iter().map(|&i| &paras[i]).collect();
                if cancel.load(Ordering::Relaxed) {
                    return (members, None);
                }
                let user = model::check_prompt(facts, &members, want_facts);
                let answer = self.proof_ask(chat, model::CHECK_SYSTEM, user).await;
                (members, Some(answer))
            })
            .buffer_unordered(concurrency);
        while let Some((members, answer)) = stream.next().await {
            let Some(answer) = answer else { continue };
            report.model_calls += 1;
            done += 1;
            let parsed = match answer {
                Ok(text) => model::parse_check(&text, &members)
                    .ok_or_else(|| "模型没有返回校对 JSON".to_string()),
                Err(e) => Err(e.to_string()),
            };
            let (new_issues, new_facts) = match parsed {
                Ok(found) => found,
                Err(e) => {
                    report
                        .failures
                        .push(format!("{}：{e}", range_label(&members)));
                    progress(ProofProgress::step(ProofStage::Model, done, total));
                    continue;
                }
            };
            progress(ProofProgress {
                found: shown(&new_issues),
                ..ProofProgress::step(ProofStage::Model, done, total)
            });
            let mut per: HashMap<usize, Cached> = HashMap::new();
            for x in &new_issues {
                per.entry(x.paragraph).or_default().issues.push(x.clone());
            }
            for f in &new_facts {
                per.entry(f.paragraph).or_default().facts.push(f.clone());
            }
            let entries: Vec<(String, String)> = members
                .iter()
                .map(|p| {
                    let c = per.remove(&p.index).unwrap_or_default();
                    (para_hash(p), serde_json::to_string(&c).unwrap_or_default())
                })
                .collect();
            self.store()
                .save_proofread_cache(&options.doc_key, &checks_key, &entries)?;
            issues.extend(new_issues);
            found_facts.extend(new_facts);
        }
        Ok(found_facts)
    }

    /// Compares figures across the document and asks the model which
    /// differing groups are real contradictions.
    #[allow(clippy::too_many_arguments)]
    async fn proof_consistency(
        &self,
        paras: &[ProofParagraph],
        found: &[Fact],
        options: &ProofOptions,
        chat: &Target,
        issues: &mut Vec<Issue>,
        report: &mut ProofReport,
        progress: &(impl Fn(ProofProgress) + Send + Sync),
    ) -> Result<()> {
        let mut groups = model::find_conflicts(found);
        groups.truncate(MAX_GROUPS);
        if groups.is_empty() {
            return Ok(());
        }
        progress(ProofProgress::step(ProofStage::Consistency, 0, 1));
        let user = model::confirm_prompt(&groups, paras);
        let key = hash(&[&user]);
        let checks = format!("{CACHE_VERSION}|confirm");
        let cached = self.store().proofread_cached(
            &options.doc_key,
            std::slice::from_ref(&key),
            &checks,
            now() - CACHE_DAYS * 86_400,
        )?;
        let answer = match cached.get(&key) {
            Some(a) => a.clone(),
            None => {
                report.model_calls += 1;
                match self.proof_ask(chat, model::CONFIRM_SYSTEM, user).await {
                    Ok(a) => {
                        self.store().save_proofread_cache(
                            &options.doc_key,
                            &checks,
                            &[(key, a.clone())],
                        )?;
                        a
                    }
                    Err(e) => {
                        report.failures.push(format!("前后一致性确认：{e}"));
                        return Ok(());
                    }
                }
            }
        };
        for (i, reason) in model::parse_confirm(&answer, groups.len()) {
            issues.extend(model::conflict_issues(&groups[i], &reason, paras));
        }
        progress(ProofProgress::step(ProofStage::Consistency, 1, 1));
        Ok(())
    }

    /// Looks up each cited law, standard and policy and asks the model
    /// whether it is still current.
    #[allow(clippy::too_many_arguments)]
    async fn proof_citations(
        &self,
        paras: &[ProofParagraph],
        lookup: &dyn CitationLookup,
        chat: &Target,
        concurrency: usize,
        issues: &mut Vec<Issue>,
        report: &mut ProofReport,
        progress: &(impl Fn(ProofProgress) + Send + Sync),
        cancel: &AtomicBool,
    ) -> Result<()> {
        let todo: Vec<usize> = report
            .citations
            .iter()
            .enumerate()
            .filter(|(_, c)| c.citation.kind != CitationKind::Unknown)
            .map(|(i, _)| i)
            .take(MAX_CITATIONS)
            .collect();
        let total = todo.len();
        let checks = format!("{CACHE_VERSION}|citation");
        let key_of = |c: &Citation| {
            hash(&[
                &c.title,
                c.standard_no.as_deref().unwrap_or(""),
                c.doc_no.as_deref().unwrap_or(""),
            ])
        };
        let keys: Vec<String> = todo
            .iter()
            .map(|&i| key_of(&report.citations[i].citation))
            .collect();
        // Citation statuses do not depend on the document.
        let cached = self.store().proofread_cached(
            "",
            &keys,
            &checks,
            now() - CITATION_CACHE_DAYS * 86_400,
        )?;
        let mut done = 0;
        let mut pending = Vec::new();
        for (&i, key) in todo.iter().zip(&keys) {
            match cached
                .get(key)
                .and_then(|s| serde_json::from_str::<CitationStatus>(s).ok())
            {
                Some(st) => {
                    report.citations[i].status = Some(st);
                    done += 1;
                }
                None => pending.push((i, report.citations[i].citation.clone())),
            }
        }
        progress(ProofProgress::step(ProofStage::Citations, done, total));
        let mut stream = futures::stream::iter(pending)
            .map(|(i, citation)| async move {
                if cancel.load(Ordering::Relaxed) {
                    return (i, citation, None);
                }
                let result = async {
                    let snippets = lookup.lookup(&citation).await?;
                    let answer = self
                        .proof_ask(
                            chat,
                            model::CITATION_SYSTEM,
                            model::citation_prompt(&citation, &snippets),
                        )
                        .await?;
                    model::parse_citation_status(&answer)
                        .ok_or_else(|| Error::Invalid("模型没有返回引用状态 JSON".into()))
                }
                .await;
                (i, citation, Some(result))
            })
            .buffer_unordered(concurrency);
        while let Some((i, citation, result)) = stream.next().await {
            let Some(result) = result else { continue };
            report.model_calls += 1;
            done += 1;
            progress(ProofProgress::step(ProofStage::Citations, done, total));
            match result {
                Ok(st) => {
                    self.store().save_proofread_cache(
                        "",
                        &checks,
                        &[(key_of(&citation), serde_json::to_string(&st)?)],
                    )?;
                    report.citations[i].status = Some(st);
                }
                Err(e) => report
                    .failures
                    .push(format!("核查《{}》：{e}", citation.title)),
            }
        }
        for c in &report.citations {
            if let Some(st) = &c.status {
                issues.extend(model::citation_issues(&c.citation, st, paras));
            }
        }
        Ok(())
    }
}
