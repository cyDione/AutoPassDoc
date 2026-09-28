//! Times opening, viewing and saving a document.
//!
//! cargo run --release -p docx-engine --example bench_open -- [file.docx] [max_open_ms]
//! Without a file, a 220k-character document with 500 comments is generated in memory.
//! With `max_open_ms`, exits non-zero when opening takes longer (used in CI).

use std::time::Instant;

use docx_engine::Document;

fn main() {
    let mut args = std::env::args().skip(1);
    let bytes = match args.next().filter(|a| a != "-") {
        Some(path) => std::fs::read(path).unwrap(),
        None => docx_engine::testgen::generate(&Default::default()),
    };
    let max_open_ms: Option<u128> = args.next().map(|v| v.parse().unwrap());
    println!("file size: {:.1} MB", bytes.len() as f64 / 1e6);

    let t = Instant::now();
    let doc = Document::from_bytes(bytes).unwrap();
    let open_ms = t.elapsed().as_millis();

    let t = Instant::now();
    let summary = doc.summary();
    let summary_ms = t.elapsed().as_millis();

    let t = Instant::now();
    let window = doc.blocks_view(0, 200);
    let view_ms = t.elapsed().as_micros();

    let t = Instant::now();
    let saved = doc.to_bytes().unwrap();
    let save_ms = t.elapsed().as_millis();

    println!(
        "chars: {}  paragraphs: {}  blocks: {}  comments: {}  outline: {}",
        summary.char_count,
        summary.paragraph_count,
        summary.block_count,
        summary.comments.len(),
        summary.outline.len()
    );
    println!(
        "open: {open_ms} ms  summary: {summary_ms} ms  200-block view: {view_ms} µs ({} blocks)  save: {save_ms} ms ({:.1} MB)",
        window.len(),
        saved.len() as f64 / 1e6
    );
    if let Some(max) = max_open_ms
        && open_ms + summary_ms > max
    {
        eprintln!(
            "open + summary took {} ms, limit {max} ms",
            open_ms + summary_ms
        );
        std::process::exit(1);
    }
}
