//! Exports a generated review document as JSON so the UI can run in a plain
//! browser (without Tauri) for development and screenshots.
//!
//! cargo run --release -p docx-engine --example export_demo -- ui/public/demo

fn main() {
    let dir = std::path::PathBuf::from(
        std::env::args()
            .nth(1)
            .unwrap_or_else(|| "ui/public/demo".into()),
    );
    std::fs::create_dir_all(&dir).unwrap();
    let bytes = docx_engine::testgen::generate(&Default::default());
    let doc = docx_engine::Document::from_bytes(bytes).unwrap();
    let summary = doc.summary();
    let blocks = doc.blocks_view(0, summary.block_count);
    std::fs::write(
        dir.join("summary.json"),
        serde_json::to_vec(&summary).unwrap(),
    )
    .unwrap();
    std::fs::write(
        dir.join("blocks.json"),
        serde_json::to_vec(&blocks).unwrap(),
    )
    .unwrap();
    let image = doc
        .paragraphs
        .iter()
        .flat_map(|p| &p.runs)
        .find_map(|r| match &r.inline {
            docx_engine::Inline::Image { rel_id: Some(id) } => Some(id.clone()),
            _ => None,
        });
    if let Some(id) = image {
        let (png, _) = doc.image(&id).unwrap().unwrap();
        std::fs::write(dir.join(format!("{id}.png")), png).unwrap();
    }
    println!("exported {} blocks to {}", blocks.len(), dir.display());
}
