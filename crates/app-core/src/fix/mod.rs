//! The AI-fix pipeline: gather the comment's context, retrieve supporting
//! passages and the reviewer's history, ask the chat model for a minimal
//! rewrite, score it with the decision model, and apply it on request.

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
use context::{FixInput, FixMode};
use judge::Judgement;
use prompt::{Passage, PromptInput};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Stage {
    Context,
    Retrieve,
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
        let citations = if settings.fix.use_kb {
            let query = format!("{}\n{}", input.comment, input.quote);
            let (found, notes) =
                knowledge::retrieve(self, &query, settings.fix.kb_passages).await?;
            warnings.extend(notes);
            found
        } else {
            Vec::new()
        };
        let passages: Vec<Passage> = citations
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
            })
            .collect();
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
        let (user, included) = prompt::build(
            &PromptInput {
                input: &input,
                reviewer: reviewer_line.as_deref(),
                profile: profile_text.as_deref(),
                passages: &passages,
                examples: &examples,
            },
            budget,
        );
        if included.passages < passages.len() {
            warnings.push(format!(
                "模型上下文有限，只用了 {} 条参考资料中的 {} 条",
                passages.len(),
                included.passages
            ));
        }
        let mut messages = vec![
            Message::system(prompt::system(input.mode)),
            Message::user(user),
        ];
        let mut rewrite = None;
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
                Ok(r) => {
                    rewrite = Some(r);
                    break;
                }
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
        let rewrite =
            rewrite.ok_or_else(|| Error::Invalid(format!("模型的修改无法使用：{last_problem}")))?;
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
