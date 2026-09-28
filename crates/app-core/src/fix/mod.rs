//! The AI-fix pipeline: gather the comment's context, retrieve supporting
//! passages and the reviewer's history, ask the chat model for a minimal
//! rewrite, score it with the decision model, and apply it on request.

pub mod context;
pub mod parse;
pub mod prompt;
