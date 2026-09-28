//! Prints what the engine sees in a .docx: outline, comments and the first blocks.
//!
//! cargo run -p docx-engine --example inspect -- file.docx [blocks]

use docx_engine::Document;
use docx_engine::view::BlockView;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: inspect <file.docx> [blocks]");
    let blocks: usize = args.next().map_or(15, |v| v.parse().unwrap());
    let doc = Document::open(&path).unwrap_or_else(|e| panic!("{e}"));
    let s = doc.summary();
    println!(
        "chars {}  paragraphs {}  blocks {}  comments {}",
        s.char_count,
        s.paragraph_count,
        s.block_count,
        s.comments.len()
    );
    println!("-- outline");
    for o in s.outline.iter().take(20) {
        println!("{}{}", "  ".repeat(o.level as usize), o.text);
    }
    println!("-- comments");
    for c in s.comments.iter().take(20) {
        let reply = c
            .parent_id
            .as_ref()
            .map(|p| format!(" (reply to {p})"))
            .unwrap_or_default();
        println!(
            "#{} {}{}{}: {}  「{}」 block {:?}",
            c.id,
            c.author,
            reply,
            if c.done { " [done]" } else { "" },
            c.text,
            c.quote,
            c.block_index
        );
    }
    println!("-- blocks");
    for b in doc.blocks_view(0, blocks) {
        match b {
            BlockView::Paragraph { index, paragraph } => {
                let text: String = paragraph.spans.iter().map(|s| s.text.as_str()).collect();
                println!(
                    "[{index}] h{:?} {}{}",
                    paragraph.heading_level,
                    paragraph.list_label.unwrap_or_default(),
                    text
                );
            }
            BlockView::Table { index, rows } => println!("[{index}] table {} rows", rows.len()),
        }
    }
}
