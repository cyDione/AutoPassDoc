//! Applies sample edits to a .docx for checking the result in Word or
//! LibreOffice: rewrites the first sentence of the paragraphs under the
//! first few comments as tracked changes, replies to and resolves those
//! comments, then saves.
//!
//! `cargo run --example edit_roundtrip -- in.docx out.docx [count]`

use docx_engine::{Document, EditMode, EditOptions, Revision};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [input, output, rest @ ..] = args.as_slice() else {
        eprintln!("usage: edit_roundtrip in.docx out.docx [count]");
        std::process::exit(2);
    };
    let count: usize = rest.first().and_then(|c| c.parse().ok()).unwrap_or(5);
    let mut doc = Document::open(input)?;
    let opts = EditOptions::new(EditMode::Tracked, "AutoPassDoc");

    let targets: Vec<(String, usize)> = doc
        .comments
        .iter()
        .filter(|c| c.parent_id.is_none() && !c.done)
        .filter_map(|c| c.anchor.as_ref().map(|a| (c.id.clone(), a.start_paragraph)))
        .filter(|(_, p)| {
            doc.paragraphs[*p]
                .runs
                .iter()
                .all(|r| r.revision == Revision::None)
        })
        .take(count)
        .collect();
    for (id, paragraph) in &targets {
        let old = doc.editable_text(*paragraph);
        let new = match old.find('。') {
            Some(end) => format!("经核实，{}（已补充依据）{}", &old[..end], &old[end..]),
            None => format!("{old}（已补充依据）"),
        };
        doc.replace_paragraphs(&[(*paragraph, new)], &opts, "AI 修复")?;
        doc.add_reply(id, "AutoPassDoc", Some("AI"), "已按意见修改。")?;
        doc.set_comments_done(&[id], true)?;
        println!("comment {id}: paragraph {paragraph} edited, replied, resolved");
    }
    doc.save(output)?;
    println!("saved {output}");
    Ok(())
}
