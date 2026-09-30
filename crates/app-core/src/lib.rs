//! AutoPassDoc application logic shared by the desktop shell: settings and
//! API keys, model providers, reviewers, the AI-fix pipeline and the
//! knowledge-base glue.

pub mod backup;
pub mod core;
pub mod dataset;
pub mod enhanced;
pub mod error;
pub mod fix;
pub mod knowledge;
pub mod prereview;
pub mod profiles;
pub mod proofread;
pub mod providers;
pub mod reviewers;
pub mod search_api;
pub mod secrets;
pub mod settings;
pub mod store;
pub mod update;
pub mod web;

pub use crate::core::{Core, RoleName, Target};
pub use error::{Error, Result};
