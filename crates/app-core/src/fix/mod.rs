//! The AI-fix pipeline: gather the comment's context, retrieve supporting
//! passages (knowledge base, related sections of the document, public
//! material on the web) and the reviewer's history, ask the chat model for a
//! minimal rewrite, look up public gaps it left and rewrite once more, score
//! it with the decision model, and apply it on request.

pub mod context;
pub mod judge;
pub mod parse;
pub mod prompt;

use std::time::Instant;

use docx_engine::diff::{DiffSpan, diff};
use docx_engine::{Document, EditOptions};
use models::{ChatRequest, Message};
use serde::Serialize;

use crate::core::{Core, RoleName, Target};
use crate::error::{Error, Result};
use crate::knowledge::{self, Citation};
use crate::profiles;
use crate::reviewers;
use crate::settings::DecisionBackend;
use crate::store::{Case, CaseAction};
use crate::web::{WebMode, WebSource};
use context::{FixInput, FixMode};
use judge::Judgement;
use parse::Rewrite;
use prompt::{Passage, PromptInput};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Stage {
    Context,
    Retrieve,
    /// Looking up public material on the web.
    Web,
    Generate,
    Judge,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FixParagraph {
    pub index: usize,
    pub old: String,
    pub new: String,
    pub diff: Vec<DiffSpan>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ContextUsed {
    pub passages: usize,
    /// Paragraphs from other sections of the document.
    pub related: usize,
    pub examples: usize,
    pub profile: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Proposal {
    pub id: String,
    /// Filled in by the shell, which knows its document ids.
    pub doc_id: u64,
    pub comment_id: String,
    pub reviewer: Option<String>,
    pub paragraphs: Vec<FixParagraph>,
    pub explanation: String,
    pub citations: Vec<Citation>,
    pub judge: Option<Judgement>,
    pub judge_error: Option<String>,
    pub model: String,
    pub elapsed_ms: u64,
    pub warnings: Vec<String>,
    pub context: ContextUsed,
    pub mode: FixMode,
}

/// The mark the model leaves where it lacks facts; a rewrite holding one
/// cannot be applied until the user fills it in.
pub const PLACEHOLDER: &str = "【待补充";

/// Written into a placeholder for the project's own data, which only the
/// report's authors have; such gaps are not looked up online.
pub const FROM_AUTHORS: &str = "编制单位";

/// Words in a comment or revision direction that point at public material
/// (policies, plans, laws, standards) worth looking up before the fix.
const PUBLIC_HINTS: &[&str] = &[
    "《",
    "规划",
    "纲要",
    "政策",
    "条例",
    "法规",
    "法律",
    "标准",
    "十四五",
    "十五五",
    "行动计划",
    "指导意见",
    "实施意见",
    "文件精神",
    "上位",
];

/// Public gaps looked up after the first draft.
const GAPS_TO_LOOK_UP: usize = 2;

/// Whether `ask` names public material to look up.
fn asks_public(ask: &str) -> bool {
    PUBLIC_HINTS.iter().any(|h| ask.contains(h))
}

/// What each "【待补充…】" asks for, with the text around it, leaving out
/// the project's own data.
pub fn public_gaps(paragraphs: &[String]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for p in paragraphs {
        let chars: Vec<char> = p.chars().collect();
        let mut rest = p.as_str();
        let mut offset = 0;
        while let Some(at) = rest.find(PLACEHOLDER) {
            let tail = &rest[at + PLACEHOLDER.len()..];
            let Some(end) = tail.find('】') else { break };
            let need = tail[..end]
                .trim_start_matches(['：', ':'])
                .trim()
                .to_string();
            let start = p[..offset + at].chars().count();
            let stop = start + PLACEHOLDER.chars().count() + tail[..end].chars().count() + 1;
            let passage: String = chars[start.saturating_sub(160)..(stop + 60).min(chars.len())]
                .iter()
                .collect();
            if !need.is_empty()
                && !need.contains(FROM_AUTHORS)
                && !out.iter().any(|(n, _)| *n == need)
            {
                out.push((need, passage));
            }
            let used = at + PLACEHOLDER.len() + end + '】'.len_utf8();
            offset += used;
            rest = &rest[used..];
        }
    }
    out
}

/// A web page as a citation, numbered after the knowledge-base passages.
fn web_citation(n: usize, s: &WebSource) -> Citation {
    Citation {
        n,
        doc_id: 0,
        chunk_id: 0,
        file_name: s.site.clone(),
        title: s.title.clone(),
        heading_path: Vec::new(),
        text: s.text.chars().take(300).collect(),
        parent_text: s.text.clone(),
        stored_path: String::new(),
        char_start: 0,
        char_end: 0,
        url: s.url.clone(),
    }
}

/// Adds the pages not cited yet; returns how many were new.
fn add_sources(citations: &mut Vec<Citation>, found: Vec<WebSource>) -> usize {
    let mut added = 0;
    for s in found {
        let known = citations.iter().any(|c| {
            c.url.is_some() && c.url == s.url
                || c.url.is_none() && s.url.is_none() && c.text == s.text
        });
        if !known {
            citations.push(web_citation(citations.len() + 1, &s));
            added += 1;
        }
    }
    added
}

fn passages_of(citations: &[Citation]) -> Vec<Passage> {
    citations
        .iter()
        .map(|c| Passage {
            n: c.n,
            title: if c.title.is_empty() {
                c.file_name.clone()
            } else {
                c.title.clone()
            },
            heading_path: c.heading_path.clone(),
            text: if c.parent_text.is_empty() {
                c.text.clone()
            } else {
                c.parent_text.clone()
            },
            url: c.url.clone(),
        })
        .collect()
}

/// A proposal waiting for the user's decision.
pub(crate) struct Stored {
    proposal: Proposal,
    case_id: i64,
    reviewer_id: Option<i64>,
}

/// One comment to fix, gathered from the document under a read lock.
pub struct FixJob {
    /// Identifies the document for per-document author mappings.
    pub doc_key: String,
    pub doc_name: String,
    pub input: FixInput,
}

fn new_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!("fix-{nanos:x}-{}", NEXT.fetch_add(1, Ordering::Relaxed))
}

fn max_tokens(target: &Target, input: &FixInput) -> u32 {
    let chars: usize = input.paragraphs.iter().map(|p| p.1.chars().count()).sum();
    let mut want = (chars * 3 + 1024).max(2048) as u32;
    if target
        .thinking
        .is_some_and(|t| t != models::ThinkingLevel::Off)
    {
        want += 8192;
    } else if target.profile.reasoning != models::Reasoning::None {
        // Some gateways ignore the off switch and the model still reasons.
        want += 4096;
    }
    want.min(target.profile.max_output_tokens.max(1024))
}

impl Core {
    /// Proposes a fix for one comment. `check` validates the rewrite against
    /// the current document (placeholders kept, no overlap with existing
    /// revisions); `progress` reports each stage.
    pub async fn propose(
        &self,
        job: FixJob,
        check: impl Fn(&[(usize, String)]) -> Result<()>,
        progress: impl Fn(Stage),
    ) -> Result<Proposal> {
        let started = Instant::now();
        progress(Stage::Context);
        let settings = self.settings()?;
        let chat = self.require(RoleName::Chat)?;
        let FixJob {
            doc_key,
            doc_name,
            input,
        } = job;
        let reviewer = {
            let store = self.store();
            let map = reviewers::resolve_all(&store, &doc_key)?;
            reviewers::lookup(&map, &store.reviewers()?, &input.author, &input.initials)
        };
        let mut warnings = Vec::new();

        progress(Stage::Retrieve);
        let mut ask = input.comment.clone();
        if let Some(d) = &input.direction {
            ask = format!("{d}\n{ask}");
        }
        let mut citations = if settings.fix.use_kb {
            let query = format!("{ask}\n{}", input.quote);
            let (found, notes) =
                knowledge::retrieve(self, &query, settings.fix.kb_passages).await?;
            warnings.extend(notes);
            found
        } else {
            Vec::new()
        };
        let (profile_text, examples) = match &reviewer {
            Some(r) => {
                let store = self.store();
                (
                    profiles::prompt_text(&store, r.id)?,
                    profiles::examples(&store, r.id, &input.comment, 3)?,
                )
            }
            None => (None, Vec::new()),
        };

        // Public material named by the comment or the direction, and what
        // the user already found, go in before the first draft.
        let web_on = settings.fix.use_web && settings.web.mode != WebMode::Off;
        if !input.sources.is_empty() {
            progress(Stage::Web);
            let given = self.given_sources(&input.sources, &ask).await;
            add_sources(&mut citations, given);
        }
        if web_on && asks_public(&ask) {
            progress(Stage::Web);
            let need = input
                .direction
                .clone()
                .unwrap_or_else(|| input.comment.clone());
            let passage = input
                .paragraphs
                .first()
                .map(|(_, t)| t.as_str())
                .or(Some(input.quote.as_str()));
            self.look_up(&need, passage, &mut citations, &mut warnings)
                .await;
        }

        progress(Stage::Generate);
        let out_tokens = max_tokens(&chat, &input);
        let budget = (chat.profile.context_window as usize * 7 / 10)
            .saturating_sub(out_tokens as usize)
            .max(2_000);
        let reviewer_line = reviewer.as_ref().map(|r| {
            if r.note.is_empty() {
                r.name.clone()
            } else {
                format!("{}（{}）", r.name, r.note)
            }
        });
        let build = |passages: &[Passage]| {
            prompt::build(
                &PromptInput {
                    input: &input,
                    reviewer: reviewer_line.as_deref(),
                    profile: profile_text.as_deref(),
                    passages,
                    examples: &examples,
                },
                budget,
            )
        };
        let mut passages = passages_of(&citations);
        let (user, mut included) = build(&passages);
        let mut rewrite = self
            .write_fix(&chat, &input, user, out_tokens, &check)
            .await?;

        // Public gaps left in the draft are looked up and the fix is
        // written again with what was found.
        let gaps = public_gaps(&rewrite.paragraphs);
        if web_on && !gaps.is_empty() {
            progress(Stage::Web);
            let before = citations.len();
            for (need, passage) in gaps.iter().take(GAPS_TO_LOOK_UP) {
                self.look_up(need, Some(passage), &mut citations, &mut warnings)
                    .await;
            }
            if citations.len() > before {
                progress(Stage::Generate);
                passages = passages_of(&citations);
                let (user, again) = build(&passages);
                match self
                    .write_fix(&chat, &input, user, out_tokens, &check)
                    .await
                {
                    Ok(r) => {
                        rewrite = r;
                        included = again;
                    }
                    Err(e) => warnings.push(format!("用查到的资料重写失败，保留初稿：{e}")),
                }
            } else {
                warnings.push(
                    "联网没有查到可以填补“待补充”的公开资料，可以点“查找资料”换个说法再查".into(),
                );
            }
        }
        if included.passages < passages.len() {
            warnings.push(format!(
                "模型上下文有限，只用了 {} 条参考资料中的 {} 条",
                passages.len(),
                included.passages
            ));
        }
        if rewrite
            .paragraphs
            .iter()
            .zip(&input.paragraphs)
            .all(|(new, (_, old))| new == old)
        {
            warnings.push("模型认为这段文字不需要修改".into());
        }
        let cited: Vec<Citation> = citations
            .into_iter()
            .filter(|c| rewrite.citations.contains(&c.n))
            .collect();

        progress(Stage::Judge);
        let (judge, judge_error) = match self.target(RoleName::Decision)? {
            None => (None, Some("尚未配置决策模型".to_string())),
            Some(decision) => {
                let threshold = reviewer
                    .as_ref()
                    .and_then(|r| r.threshold)
                    .unwrap_or(settings.fix.threshold);
                let state =
                    judge::state(&input, &rewrite.paragraphs, &rewrite.explanation, &passages);
                let questions = judge::questions();
                let (answers, backend) = match settings.roles.decision_backend {
                    DecisionBackend::Jev => (
                        self.client()
                            .decide(&decision.provider, &decision.model, &state, &questions)
                            .await,
                        "jev",
                    ),
                    DecisionBackend::Chat => (
                        self.client()
                            .decide_via_chat(
                                &decision.provider,
                                &decision.model,
                                &decision.profile,
                                &state,
                                &questions,
                            )
                            .await,
                        "chat",
                    ),
                };
                match answers
                    .map_err(|e| format!("决策模型调用失败：{e}"))
                    .and_then(|a| judge::score(&a, input.mode, threshold, backend, &decision.model))
                {
                    Ok(j) => (Some(j), None),
                    Err(e) => (None, Some(e)),
                }
            }
        };

        let paragraphs: Vec<FixParagraph> = input
            .paragraphs
            .iter()
            .zip(&rewrite.paragraphs)
            .map(|((index, old), new)| FixParagraph {
                index: *index,
                diff: diff(old, new),
                old: old.clone(),
                new: new.clone(),
            })
            .collect();
        let proposal = Proposal {
            id: new_id(),
            doc_id: 0,
            comment_id: input.comment_id.clone(),
            reviewer: reviewer.as_ref().map(|r| r.name.clone()),
            paragraphs,
            explanation: rewrite.explanation,
            citations: cited,
            judge_error,
            model: chat.model.clone(),
            elapsed_ms: started.elapsed().as_millis() as u64,
            warnings,
            context: ContextUsed {
                passages: included.passages,
                related: included.related,
                examples: included.examples,
                profile: included.profile,
            },
            mode: input.mode,
            judge,
        };

        let case_id = self.store().add_case(&Case {
            id: 0,
            reviewer_id: reviewer.as_ref().map(|r| r.id),
            doc_key,
            doc_name,
            comment_id: input.comment_id.clone(),
            author: input.author.clone(),
            // The direction is feedback on how the reviewer's comment was
            // meant; profiles learn from it along with the comment.
            comment: match input.direction.as_deref().map(str::trim) {
                Some(d) if !d.is_empty() => format!("{}\n【修改方向】{d}", input.comment),
                _ => input.comment.clone(),
            },
            original: join(proposal.paragraphs.iter().map(|p| p.old.as_str())),
            suggestion: join(proposal.paragraphs.iter().map(|p| p.new.as_str())),
            final_text: None,
            action: CaseAction::Pending,
            confidence: proposal.judge.as_ref().map(|j| j.confidence),
            judge: proposal
                .judge
                .as_ref()
                .map(serde_json::to_string)
                .transpose()?,
            category: proposal.judge.as_ref().and_then(|j| j.category.clone()),
            created_at: 0,
        })?;
        self.proposals.lock().unwrap().insert(
            proposal.id.clone(),
            Stored {
                proposal: proposal.clone(),
                case_id,
                reviewer_id: reviewer.map(|r| r.id),
            },
        );
        Ok(proposal)
    }

    /// Asks the chat model for the rewrite, once more with the problem
    /// pointed out when the first answer cannot be used.
    async fn write_fix(
        &self,
        chat: &Target,
        input: &FixInput,
        user: String,
        out_tokens: u32,
        check: &impl Fn(&[(usize, String)]) -> Result<()>,
    ) -> Result<Rewrite> {
        let mut messages = vec![
            Message::system(prompt::system(input.mode)),
            Message::user(user),
        ];
        let mut last_problem = String::new();
        for attempt in 0..2 {
            let mut req = ChatRequest::new(chat.model.clone(), messages.clone());
            req.profile = chat.profile.clone();
            req.thinking = chat.thinking;
            req.max_tokens = Some(out_tokens);
            req.temperature = Some(0.3);
            req.json_output = true;
            let response = self
                .client()
                .chat(&chat.provider, &req)
                .await
                .map_err(|e| Error::Invalid(format!("大语言模型调用失败：{e}")))?;
            let parsed = parse::rewrite(&response.content, input.paragraphs.len()).and_then(|r| {
                let changes: Vec<(usize, String)> = input
                    .paragraphs
                    .iter()
                    .zip(&r.paragraphs)
                    .map(|((i, _), t)| (*i, t.clone()))
                    .collect();
                check(&changes).map_err(|e| e.to_string())?;
                Ok(r)
            });
            match parsed {
                Ok(r) => return Ok(r),
                Err(problem) if attempt == 0 => {
                    messages.push(Message::assistant(response.content));
                    messages.push(Message::user(prompt::correction(
                        &problem,
                        input.paragraphs.len(),
                    )));
                    last_problem = problem;
                }
                Err(problem) => last_problem = problem,
            }
        }
        Err(Error::Invalid(format!(
            "模型的修改无法使用：{last_problem}"
        )))
    }

    /// Looks `need` up on the web and cites the pages found. Failures become
    /// warnings: the fix goes on without them.
    async fn look_up(
        &self,
        need: &str,
        passage: Option<&str>,
        citations: &mut Vec<Citation>,
        warnings: &mut Vec<String>,
    ) {
        let mut notes = Vec::new();
        match self.web_sources(need, passage, &mut notes).await {
            Ok(found) if found.is_empty() && !notes.is_empty() => {
                warnings.push(format!("联网查找“{need}”没有结果（{}）", notes.join("；")))
            }
            Ok(found) => {
                add_sources(citations, found);
            }
            Err(e) => warnings.push(format!("联网查找资料失败：{e}")),
        }
    }

    /// Writes a proposal into the document as one undo step (the rewrite,
    /// plus the reply and the resolved mark the settings ask for). Returns
    /// the reviewer whose profile is due for a refresh, if any.
    pub fn apply_fix(
        &self,
        doc: &mut Document,
        proposal_id: &str,
        edited: Option<Vec<String>>,
        force: bool,
    ) -> Result<Option<i64>> {
        let settings = self.settings()?;
        let (proposal, case_id, reviewer_id) = {
            let map = self.proposals.lock().unwrap();
            let s = map
                .get(proposal_id)
                .ok_or_else(|| Error::Invalid("这条修改建议已失效，请重新生成".into()))?;
            (s.proposal.clone(), s.case_id, s.reviewer_id)
        };
        if !force && proposal.judge.as_ref().is_some_and(|j| !j.passed) {
            return Err(Error::Invalid("置信度未达标，需要确认后才能应用".into()));
        }
        let was_edited = edited.is_some();
        let texts: Vec<String> = match edited {
            Some(t) if t.len() == proposal.paragraphs.len() => t,
            Some(_) => return Err(Error::Invalid("编辑后的段落数与原文不一致".into())),
            None => proposal.paragraphs.iter().map(|p| p.new.clone()).collect(),
        };
        if texts.iter().any(|t| t.contains(PLACEHOLDER)) {
            return Err(Error::Invalid(if was_edited {
                "修改里还有“【待补充…】”，请改成实际内容后再应用".into()
            } else {
                "修改里有待补充的内容，请先查找资料、手动补充后再应用".into()
            }));
        }
        for p in &proposal.paragraphs {
            if p.index >= doc.paragraphs.len() || doc.editable_text(p.index) != p.old {
                return Err(Error::Invalid(
                    "文档在生成建议后已被修改，请重新生成".into(),
                ));
            }
        }
        let changes: Vec<(usize, String)> = proposal
            .paragraphs
            .iter()
            .zip(&texts)
            .map(|(p, t)| (p.index, t.clone()))
            .collect();
        let options = EditOptions::new(settings.fix.edit_mode, settings.fix.author.clone());
        let label = match &proposal.reviewer {
            Some(r) => format!("AI 修复（{r}）"),
            None => "AI 修复".to_string(),
        };
        let fix = &settings.fix;
        doc.group(&label, |doc| {
            doc.replace_paragraphs(&changes, &options, &label)?;
            if fix.reply_on_apply && !fix.reply_text.trim().is_empty() {
                doc.add_reply(&proposal.comment_id, &fix.author, None, &fix.reply_text)?;
            }
            if fix.resolve_on_apply {
                doc.set_comments_done(&[&proposal.comment_id], true)?;
            }
            Ok(())
        })?;

        let suggested: Vec<&str> = proposal.paragraphs.iter().map(|p| p.new.as_str()).collect();
        let action = if texts.iter().map(String::as_str).eq(suggested) {
            CaseAction::Accepted
        } else {
            CaseAction::Edited
        };
        self.store().set_case_outcome(
            case_id,
            action,
            Some(&join(texts.iter().map(String::as_str))),
        )?;
        self.proposals.lock().unwrap().remove(proposal_id);
        self.profile_due(reviewer_id)
    }

    /// Records that the user dismissed a proposal.
    pub fn reject_fix(&self, proposal_id: &str) -> Result<Option<i64>> {
        let Some(stored) = self.proposals.lock().unwrap().remove(proposal_id) else {
            return Ok(None);
        };
        self.store()
            .set_case_outcome(stored.case_id, CaseAction::Rejected, None)?;
        self.profile_due(stored.reviewer_id)
    }

    fn profile_due(&self, reviewer_id: Option<i64>) -> Result<Option<i64>> {
        let Some(id) = reviewer_id else {
            return Ok(None);
        };
        let every = self.settings()?.fix.profile_every.max(1);
        Ok((self.store().cases_since_profile(id)? >= every).then_some(id))
    }
}

fn join<'a>(parts: impl Iterator<Item = &'a str>) -> String {
    parts.collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_gaps_leave_out_the_authors_data() {
        let gaps = public_gaps(&[
            "本项目衔接【待补充：美丽上海“十五五”规划目标】，按【待补充：需编制单位提供测算方法】测算。".into(),
            "又见【待补充：美丽上海“十五五”规划目标】和【待补充】。".into(),
        ]);
        assert_eq!(gaps.len(), 1, "{gaps:?}");
        assert_eq!(gaps[0].0, "美丽上海“十五五”规划目标");
        assert!(gaps[0].1.starts_with("本项目衔接【待补充"));
        assert!(asks_public("加入“美丽上海 十五五”的简述"));
        assert!(!asks_public("请说明测算方法和主要参数取值依据。"));
    }
}
