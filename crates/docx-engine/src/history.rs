//! Undo and redo. Each step stores, per changed part, only the bytes between
//! the common prefix and suffix of the old and new content, so a one-word
//! edit in a 5 MB `document.xml` costs a few hundred bytes.

use crate::error::{Error, Result};
use crate::package::Package;

const LIMIT: usize = 200;

struct PartPatch {
    part: String,
    prefix: usize,
    old: Vec<u8>,
    new: Vec<u8>,
    /// The part did not exist before this step.
    created: bool,
}

impl PartPatch {
    fn between(part: &str, old: Option<&[u8]>, new: &[u8]) -> Self {
        let Some(old) = old else {
            return Self {
                part: part.to_string(),
                prefix: 0,
                old: Vec::new(),
                new: new.to_vec(),
                created: true,
            };
        };
        let prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
        let suffix = old[prefix..]
            .iter()
            .rev()
            .zip(new[prefix..].iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        Self {
            part: part.to_string(),
            prefix,
            old: old[prefix..old.len() - suffix].to_vec(),
            new: new[prefix..new.len() - suffix].to_vec(),
            created: false,
        }
    }

    fn swap(&self, package: &mut Package, forward: bool) -> Result<()> {
        if self.created {
            if forward {
                package.set_part(&self.part, self.new.clone())?;
            } else {
                package.remove_part(&self.part);
            }
            return Ok(());
        }
        let (from, to) = if forward {
            (&self.old, &self.new)
        } else {
            (&self.new, &self.old)
        };
        let current = package.required_part(&self.part)?;
        let end = self.prefix + from.len();
        if current.len() < end || current[self.prefix..end] != from[..] {
            return Err(Error::Invalid(format!("撤销记录与 {} 不一致", self.part)));
        }
        let mut data = Vec::with_capacity(current.len() - from.len() + to.len());
        data.extend_from_slice(&current[..self.prefix]);
        data.extend_from_slice(to);
        data.extend_from_slice(&current[end..]);
        package.set_part(&self.part, data)
    }
}

pub(crate) struct Step {
    id: u64,
    label: String,
    patches: Vec<PartPatch>,
}

impl Step {
    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn apply(&self, package: &mut Package) -> Result<()> {
        self.patches.iter().try_for_each(|p| p.swap(package, true))
    }

    pub fn revert(&self, package: &mut Package) -> Result<()> {
        self.patches
            .iter()
            .rev()
            .try_for_each(|p| p.swap(package, false))
    }
}

#[derive(Default)]
pub(crate) struct History {
    undo: Vec<Step>,
    redo: Vec<Step>,
    next_id: u64,
    /// Id of the newest undo step when the document was last saved; 0 = never edited.
    saved: u64,
}

impl History {
    /// Writes `parts` into the package and returns the step that undoes it.
    pub fn record(
        &mut self,
        package: &mut Package,
        label: &str,
        parts: Vec<(String, Vec<u8>)>,
    ) -> Result<Step> {
        let mut patches = Vec::with_capacity(parts.len());
        for (name, data) in parts {
            let old = package.part(&name)?;
            patches.push(PartPatch::between(&name, old.as_deref(), &data));
            package.set_part(&name, data)?;
        }
        self.next_id += 1;
        Ok(Step {
            id: self.next_id,
            label: label.to_string(),
            patches,
        })
    }

    pub fn push(&mut self, step: Step) {
        self.undo.push(step);
        if self.undo.len() > LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    pub fn take_undo(&mut self) -> Option<Step> {
        self.undo.pop()
    }

    pub fn take_redo(&mut self) -> Option<Step> {
        self.redo.pop()
    }

    pub fn push_undone(&mut self, step: Step) {
        self.redo.push(step);
    }

    pub fn push_redone(&mut self, step: Step) {
        self.undo.push(step);
    }

    pub fn undo_label(&self) -> Option<&str> {
        self.undo.last().map(|s| s.label.as_str())
    }

    pub fn redo_label(&self) -> Option<&str> {
        self.redo.last().map(|s| s.label.as_str())
    }

    fn head(&self) -> u64 {
        self.undo.last().map_or(0, |s| s.id)
    }

    pub fn mark_saved(&mut self) {
        self.saved = self.head();
    }

    pub fn is_dirty(&self) -> bool {
        self.head() != self.saved
    }
}
