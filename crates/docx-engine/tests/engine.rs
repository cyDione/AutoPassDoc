use std::collections::HashSet;
use std::io::{Cursor, Read, Write};

use docx_engine::testgen::{self, AUTHORS, Spec};
use docx_engine::view::BlockView;
use docx_engine::{Block, Document, Inline, Revision};
use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

fn large() -> Vec<u8> {
    testgen::generate(&Spec::default())
}

const W: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006" xmlns:w14="http://schemas.microsoft.com/office/word/2010/wordml""#;

/// Builds a minimal .docx from a body fragment and optional comments.
fn docx(body: &str, comments: &[(&str, &str, &str)]) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let opts = SimpleFileOptions::default();
    zip.start_file("word/document.xml", opts).unwrap();
    write!(zip, r#"<?xml version="1.0" encoding="UTF-8"?><w:document {W}><w:body>{body}</w:body></w:document>"#).unwrap();
    if !comments.is_empty() {
        zip.start_file("word/comments.xml", opts).unwrap();
        write!(zip, "<w:comments {W}>").unwrap();
        for (id, author, text) in comments {
            write!(zip, r#"<w:comment w:id="{id}" w:author="{author}"><w:p><w:r><w:t>{text}</w:t></w:r></w:p></w:comment>"#).unwrap();
        }
        write!(zip, "</w:comments>").unwrap();
    }
    zip.finish().unwrap().into_inner()
}

fn parts(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
    (0..archive.len())
        .map(|i| {
            let mut f = archive.by_index(i).unwrap();
            let mut data = Vec::new();
            f.read_to_end(&mut data).unwrap();
            (f.name().to_string(), data)
        })
        .collect()
}

#[test]
fn large_document_parses_completely() {
    let doc = Document::from_bytes(large()).unwrap();
    let summary = doc.summary();

    assert!(
        summary.char_count >= 220_000,
        "only {} chars",
        summary.char_count
    );
    let top_level: Vec<_> = summary
        .comments
        .iter()
        .filter(|c| c.parent_id.is_none())
        .collect();
    assert_eq!(top_level.len(), 500);
    assert!(summary.comments.len() > 500, "expected replies");
    assert!(
        summary.comments.iter().all(|c| c.block_index.is_some()),
        "every comment is anchored"
    );
    assert!(
        top_level.iter().all(|c| !c.quote.is_empty()),
        "every comment quotes text"
    );
    assert!(summary.comments.iter().any(|c| c.done));
    let authors: HashSet<_> = top_level.iter().map(|c| c.author.as_str()).collect();
    assert!(authors.iter().all(|a| AUTHORS.contains(a)));
    assert!(authors.len() >= 6);

    // Automatic heading numbering.
    assert!(
        summary.outline[0].text.starts_with("一、第1部分"),
        "{}",
        summary.outline[0].text
    );
    assert!(
        summary.outline[1].text.starts_with("（一）"),
        "{}",
        summary.outline[1].text
    );
    assert!(
        summary.outline[2].text.starts_with("1."),
        "{}",
        summary.outline[2].text
    );
    assert_eq!(
        summary.outline.iter().filter(|o| o.level == 0).count(),
        summary.outline.len() / 17
    );

    // Tables, including a merged cell and a comment inside a cell.
    let tables: Vec<_> = doc
        .blocks
        .iter()
        .filter_map(|b| match b {
            Block::Table(t) => Some(t),
            _ => None,
        })
        .collect();
    assert!(!tables.is_empty());
    assert!(tables[0].rows.iter().flatten().any(|c| c.grid_span == 2));
    assert!(
        top_level
            .iter()
            .any(|c| matches!(doc.blocks[c.block_index.unwrap()], Block::Table(_)))
    );

    // Comments whose range crosses a paragraph boundary.
    assert!(
        doc.comments
            .iter()
            .filter_map(|c| c.anchor.as_ref())
            .any(|a| a.end_paragraph > a.start_paragraph)
    );

    // Tracked changes.
    let runs: Vec<_> = doc.paragraphs.iter().flat_map(|p| &p.runs).collect();
    assert!(
        runs.iter()
            .any(|r| r.revision == Revision::Insert
                && r.revision_author.as_deref() == Some("李教授"))
    );
    assert!(runs.iter().any(|r| r.revision == Revision::Delete));
    assert!(runs.iter().any(|r| r.format.bold));

    // The image resolves to PNG bytes.
    let image = runs
        .iter()
        .find_map(|r| match &r.inline {
            Inline::Image { rel_id } => rel_id.clone(),
            _ => None,
        })
        .unwrap();
    let (bytes, mime) = doc.image(&image).unwrap().unwrap();
    assert_eq!(mime, "image/png");
    assert!(bytes.starts_with(b"\x89PNG"));
}

#[test]
fn saving_without_edits_keeps_every_part_identical() {
    let original = large();
    let doc = Document::from_bytes(original.clone()).unwrap();
    assert!(!doc.is_modified());
    let saved = doc.to_bytes().unwrap();
    assert_eq!(parts(&original), parts(&saved));

    let reopened = Document::from_bytes(saved).unwrap();
    assert_eq!(
        serde_json::to_string(&doc.summary()).unwrap(),
        serde_json::to_string(&reopened.summary()).unwrap()
    );
}

#[test]
fn save_writes_file_atomically() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("报告_AutoPassDoc.docx");
    let mut doc = Document::from_bytes(large()).unwrap();
    doc.save(&path).unwrap();
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        1,
        "no temp file left behind"
    );
    let reopened = Document::open(&path).unwrap();
    assert_eq!(reopened.paragraphs.len(), doc.paragraphs.len());
}

#[test]
fn paragraph_spans_split_at_comment_boundaries() {
    let body = r#"<w:p><w:r><w:t>前文</w:t></w:r><w:commentRangeStart w:id="1"/><w:r><w:rPr><w:b/></w:rPr><w:t>批注</w:t></w:r><w:r><w:t>范围</w:t></w:r><w:commentRangeEnd w:id="1"/><w:r><w:commentReference w:id="1"/></w:r><w:r><w:t>后文</w:t></w:r></w:p>"#;
    let doc = Document::from_bytes(docx(body, &[("1", "张主任", "请核实")])).unwrap();
    let BlockView::Paragraph { paragraph, .. } = &doc.blocks_view(0, 1)[0] else {
        panic!()
    };
    let spans: Vec<(&str, bool, bool)> = paragraph
        .spans
        .iter()
        .map(|s| (s.text.as_str(), s.format.bold, !s.comment_ids.is_empty()))
        .collect();
    assert_eq!(
        spans,
        [
            ("前文", false, false),
            ("批注", true, true),
            ("范围", false, true),
            ("后文", false, false)
        ]
    );
    let summary = doc.summary();
    assert_eq!(summary.comments[0].quote, "批注范围");
    assert_eq!(summary.comments[0].author, "张主任");
    assert_eq!(summary.comments[0].text, "请核实");
}

#[test]
fn handles_word_markup_edge_cases() {
    let body = concat!(
        // Entities, tabs and breaks.
        r#"<w:p><w:r><w:t>A&amp;B &lt;C&gt; &#x4E2D;</w:t><w:tab/><w:t>D</w:t><w:br/><w:t>E</w:t></w:r></w:p>"#,
        // Field codes show only their result; bold explicitly turned off.
        r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText> PAGE </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:rPr><w:b w:val="0"/></w:rPr><w:t>3</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#,
        // Content control (e.g. a table of contents) and a hyperlink.
        r#"<w:sdt><w:sdtPr><w:docPartObj/></w:sdtPr><w:sdtContent><w:p><w:hyperlink w:anchor="x"><w:r><w:t>目录项</w:t></w:r></w:hyperlink></w:p></w:sdtContent></w:sdt>"#,
        // Alternate content: only the preferred choice is read.
        r#"<w:p><mc:AlternateContent><mc:Choice Requires="wps"><w:r><w:t>新</w:t></w:r></mc:Choice><mc:Fallback><w:r><w:t>旧</w:t></w:r></mc:Fallback></mc:AlternateContent></w:p>"#,
        // Direct outline level; a comment that starts between paragraphs.
        r#"<w:commentRangeStart w:id="5"/><w:p><w:pPr><w:outlineLvl w:val="1"/><w:rPr><w:b/></w:rPr></w:pPr><w:r><w:t>二级标题</w:t></w:r><w:commentRangeEnd w:id="5"/></w:p>"#,
        // A comment with only a reference mark.
        r#"<w:p><w:r><w:t>结尾</w:t></w:r><w:r><w:commentReference w:id="6"/></w:r></w:p><w:p/>"#,
        r#"<w:sectPr><w:pgSz w:w="11906"/></w:sectPr>"#,
    );
    let doc = Document::from_bytes(docx(
        body,
        &[("5", "赵工", "标题太长"), ("6", "user", "补充结论")],
    ))
    .unwrap();
    let texts: Vec<String> = doc.paragraphs.iter().map(|p| p.accepted_text()).collect();
    assert_eq!(
        texts,
        [
            "A&B <C> 中\tD\nE",
            "3",
            "目录项",
            "新",
            "二级标题",
            "结尾",
            ""
        ]
    );
    assert!(!doc.paragraphs[1].runs[0].format.bold);
    assert_eq!(doc.heading_level(4), Some(1));
    assert_eq!(doc.heading_level(0), None);

    let summary = doc.summary();
    assert_eq!(summary.outline.len(), 1);
    assert_eq!(summary.comments[0].quote, "二级标题");
    assert_eq!(summary.comments[1].paragraph_index, Some(5));
}

#[test]
fn rejects_files_that_are_not_docx() {
    assert!(Document::from_bytes(b"not a zip".to_vec()).is_err());
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file("hello.txt", SimpleFileOptions::default())
        .unwrap();
    let bytes = zip.finish().unwrap().into_inner();
    assert!(matches!(
        Document::from_bytes(bytes),
        Err(docx_engine::Error::MissingPart(_))
    ));
}

fn fixture(name: &str) -> Document {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/fixtures")
        .join(name);
    Document::open(path).unwrap()
}

/// The generated report after a round trip through LibreOffice Writer,
/// which rewrites all markup in its own style.
#[test]
fn reads_libreoffice_output_like_the_original() {
    let lo = fixture("libreoffice-roundtrip.docx").summary();
    let original = Document::from_bytes(large()).unwrap().summary();
    assert_eq!(lo.char_count, original.char_count);
    assert_eq!(lo.paragraph_count, original.paragraph_count);
    assert_eq!(lo.comments.len(), original.comments.len());
    assert_eq!(lo.outline.len(), original.outline.len());
    assert!(lo.comments.iter().all(|c| c.block_index.is_some()));
}

/// Built from python-docx's default template, which was authored by Microsoft Word.
#[test]
fn reads_word_template_document() {
    let doc = fixture("word-template.docx");
    let s = doc.summary();
    let outline: Vec<_> = s
        .outline
        .iter()
        .map(|o| (o.level, o.text.as_str()))
        .collect();
    assert_eq!(outline, [(0, "项目概况"), (1, "建设内容")]);
    assert_eq!(doc.list_label(3), Some("1."));
    assert_eq!(doc.list_label(5), Some("3."));
    assert_eq!(s.comments.len(), 2);
    assert_eq!(s.comments[0].quote, "3850万元");
    assert_eq!(s.comments[1].author, "Administrator");
    assert!(matches!(
        doc.blocks[s.comments[1].block_index.unwrap()],
        Block::Table(_)
    ));
}
