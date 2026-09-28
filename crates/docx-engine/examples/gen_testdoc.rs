//! Writes a synthetic review document for manual testing and benchmarks.
//!
//! cargo run --release -p docx-engine --example gen_testdoc -- out.docx [chars] [comments]

fn main() {
    let mut args = std::env::args().skip(1);
    let out = args
        .next()
        .unwrap_or_else(|| "testdata/generated/large-review.docx".into());
    let mut spec = docx_engine::testgen::Spec::default();
    if let Some(chars) = args.next() {
        spec.target_chars = chars.parse().expect("chars must be a number");
    }
    if let Some(comments) = args.next() {
        spec.comments = comments.parse().expect("comments must be a number");
    }
    if let Some(dir) = std::path::Path::new(&out).parent() {
        std::fs::create_dir_all(dir).unwrap();
    }
    std::fs::write(&out, docx_engine::testgen::generate(&spec)).unwrap();
    println!("wrote {out}");
}
