use std::io::{Cursor, Read, Write};

use docx_engine::testgen::{self, Spec};
use docx_engine::{Document, EditMode, EditOptions, Inline, Revision};
use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

const NS: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships""#;
const W14: &str = r#"xmlns:w14="http://schemas.microsoft.com/office/word/2010/wordml""#;

/// A complete package: content types, relationships, body and optional
/// comments.xml (without commentsExtended).
fn package(body: &str, comments: &str) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let opts = SimpleFileOptions::default();
    let mut add = |name: &str, data: String| {
        zip.start_file(name, opts).unwrap();
        zip.write_all(data.as_bytes()).unwrap();
    };
    add("[Content_Types].xml", r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/comments.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.comments+xml"/></Types>"#.into());
    add("_rels/.rels", r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#.into());
    add("word/_rels/document.xml.rels", r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/comments" Target="comments.xml"/></Relationships>"#.into());
    add(
        "word/document.xml",
        format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document {NS}><w:body>{body}<w:sectPr/></w:body></w:document>"#
        ),
    );
    if !comments.is_empty() {
        add(
            "word/comments.xml",
            format!(
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:comments {NS}>{comments}</w:comments>"#
            ),
        );
    }
    zip.finish().unwrap().into_inner()
}

fn part(doc: &Document, name: &str) -> String {
    let bytes = doc.to_bytes().unwrap();
    let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut s = String::new();
    archive
        .by_name(name)
        .unwrap()
        .read_to_string(&mut s)
        .unwrap();
    s
}

fn has_part(doc: &Document, name: &str) -> bool {
    let bytes = doc.to_bytes().unwrap();
    ZipArchive::new(Cursor::new(bytes))
        .unwrap()
        .by_name(name)
        .is_ok()
}

fn tracked() -> EditOptions {
    EditOptions {
        mode: EditMode::Tracked,
        author: "AutoPassDoc".into(),
        date: "2026-09-28T00:00:00Z".into(),
    }
}

fn direct() -> EditOptions {
    EditOptions {
        mode: EditMode::Direct,
        ..tracked()
    }
}

fn reopen(doc: &Document) -> Document {
    Document::from_bytes(doc.to_bytes().unwrap()).unwrap()
}

/// Text with deletions in [-…] and insertions in [+…].
fn marked(doc: &Document, paragraph: usize) -> String {
    let p = &doc.paragraphs[paragraph];
    let chars: Vec<char> = p.text.chars().collect();
    p.runs
        .iter()
        .map(|r| {
            let t: String = chars[r.start..r.end].iter().collect();
            match r.revision {
                Revision::None => t,
                Revision::Insert => format!("[+{t}]"),
                Revision::Delete => format!("[-{t}]"),
            }
        })
        .collect()
}

#[test]
fn tracked_edit_splits_only_the_touched_runs() {
    let body = r#"<w:p><w:r w:rsidR="00AB"><w:rPr><w:b/></w:rPr><w:t>项目总投资</w:t></w:r><w:r><w:t xml:space="preserve">为1200万元，建设周期两年。</w:t></w:r></w:p><w:p><w:r><w:t>第二段不动。</w:t></w:r></w:p>"#;
    let mut doc = Document::from_bytes(package(body, "")).unwrap();
    let before = part(&doc, "word/document.xml");
    let second = doc.paragraphs[1].span;
    let second_xml = before[second.start..second.end].to_string();

    let report = doc
        .replace_paragraphs(
            &[(0, "项目总投资为1500万元，建设周期三年。".into())],
            &tracked(),
            "AI 修复",
        )
        .unwrap();
    assert_eq!(report.paragraphs, 1);
    assert_eq!(report.deleted_chars, 5);
    assert_eq!(report.inserted_chars, 5);

    assert_eq!(doc.editable_text(0), "项目总投资为1500万元，建设周期三年。");
    assert_eq!(
        marked(&doc, 0),
        "项目总投资为[-1200][+1500]万元，建设周期[-两][+三]年。"
    );
    let p = &doc.paragraphs[0];
    assert!(p.runs[0].format.bold, "untouched bold run keeps its format");
    let ins = p
        .runs
        .iter()
        .find(|r| r.revision == Revision::Insert)
        .unwrap();
    assert_eq!(ins.revision_author.as_deref(), Some("AutoPassDoc"));

    let after = part(&doc, "word/document.xml");
    assert!(
        after.contains(r#"<w:r w:rsidR="00AB"><w:rPr><w:b/></w:rPr><w:t>项目总投资</w:t></w:r>"#)
    );
    assert!(
        after.contains(&second_xml),
        "other paragraphs keep their bytes"
    );
    assert!(after.contains(r#"w:author="AutoPassDoc" w:date="2026-09-28T00:00:00Z""#));
    assert_eq!(reopen(&doc).editable_text(0), doc.editable_text(0));
}

#[test]
fn direct_edit_leaves_no_revision_marks() {
    let body = r#"<w:p><w:r><w:rPr><w:i/></w:rPr><w:t>加强管理工作</w:t></w:r></w:p>"#;
    let mut doc = Document::from_bytes(package(body, "")).unwrap();
    doc.replace_paragraphs(&[(0, "切实加强监管工作".into())], &direct(), "改写")
        .unwrap();
    assert_eq!(doc.paragraphs[0].text, "切实加强监管工作");
    assert!(
        doc.paragraphs[0]
            .runs
            .iter()
            .all(|r| r.revision == Revision::None && r.format.italic)
    );
    assert!(!part(&doc, "word/document.xml").contains("w:ins"));
}

#[test]
fn escapes_and_converts_tabs_and_line_breaks() {
    let body = r#"<w:p><w:r><w:t>A</w:t><w:tab/><w:t>B</w:t></w:r></w:p>"#;
    let mut doc = Document::from_bytes(package(body, "")).unwrap();
    assert_eq!(doc.editable_text(0), "A\tB");
    doc.replace_paragraphs(
        &[(0, "A\tB <&> \"引号\"\n下一行".into())],
        &tracked(),
        "改写",
    )
    .unwrap();
    assert_eq!(doc.editable_text(0), "A\tB <&> \"引号\"\n下一行");
    let xml = part(&doc, "word/document.xml");
    assert!(xml.contains("&lt;&amp;&gt;"));
    assert!(xml.contains("<w:br/>"));
    assert_eq!(reopen(&doc).editable_text(0), "A\tB <&> \"引号\"\n下一行");
}

#[test]
fn keeps_comment_marks_in_place() {
    let body = r#"<w:p><w:r><w:t>本项目</w:t></w:r><w:commentRangeStart w:id="0"/><w:r><w:t>预计带动就业</w:t></w:r><w:commentRangeEnd w:id="0"/><w:r><w:rPr><w:rStyle w:val="CommentReference"/></w:rPr><w:commentReference w:id="0"/></w:r><w:r><w:t>。</w:t></w:r></w:p>"#;
    let comments = r#"<w:comment w:id="0" w:author="王处长"><w:p><w:r><w:t>请给出具体数字</w:t></w:r></w:p></w:comment>"#;
    let mut doc = Document::from_bytes(package(body, comments)).unwrap();
    doc.replace_paragraphs(
        &[(0, "本项目预计带动就业约300人。".into())],
        &tracked(),
        "AI 修复",
    )
    .unwrap();
    assert_eq!(marked(&doc, 0), "本项目预计带动就业[+约300人]。");
    let a = doc.comments[0].anchor.clone().unwrap();
    assert_eq!(
        (a.start_offset, a.end_offset),
        (3, 14),
        "the insertion joins the comment range"
    );
    let summary = doc.summary();
    assert_eq!(summary.comments[0].quote, "预计带动就业约300人");
}

#[test]
fn refuses_edits_that_touch_images_or_existing_revisions() {
    let image = r#"<w:r><w:drawing><wp:inline xmlns:wp="x"><a:blip xmlns:a="y" r:embed="rId9"/></wp:inline></w:drawing></w:r>"#;
    let body = format!(
        r#"<w:p><w:r><w:t>见下图</w:t></w:r>{image}<w:r><w:t>。</w:t></w:r></w:p><w:p><w:r><w:t>原有</w:t></w:r><w:ins w:id="1" w:author="李教授"><w:r><w:t>插入内容</w:t></w:r></w:ins><w:r><w:t>结尾</w:t></w:r></w:p>"#
    );
    let mut doc = Document::from_bytes(package(&body, "")).unwrap();
    assert_eq!(doc.editable_text(0), "见下图⟦图⟧。");
    assert!(
        doc.paragraphs[0]
            .runs
            .iter()
            .any(|r| matches!(r.inline, Inline::Image { .. }))
    );

    let before = doc.to_bytes().unwrap();
    assert!(
        doc.replace_paragraphs(&[(0, "见下图。".into())], &tracked(), "x")
            .is_err()
    );
    assert!(
        doc.replace_paragraphs(&[(0, "⟦图⟧见下图。".into())], &tracked(), "x")
            .is_err()
    );
    assert!(
        doc.replace_paragraphs(&[(1, "原有插入改动结尾".into())], &tracked(), "x")
            .is_err()
    );
    assert_eq!(
        doc.to_bytes().unwrap(),
        before,
        "failed edits change nothing"
    );
    assert!(!doc.is_dirty());

    // Text around the image and next to the revision can still change.
    doc.replace_paragraphs(&[(0, "请见下图⟦图⟧。".into())], &tracked(), "x")
        .unwrap();
    assert_eq!(doc.editable_text(0), "请见下图⟦图⟧。");
    doc.replace_paragraphs(&[(1, "原有插入内容新结尾".into())], &tracked(), "x")
        .unwrap();
    assert_eq!(marked(&doc, 1), "原有[+插入内容][+新]结尾");
}

#[test]
fn fills_empty_paragraphs() {
    let body = r#"<w:p/><w:p><w:pPr><w:jc w:val="center"/></w:pPr></w:p>"#;
    let mut doc = Document::from_bytes(package(body, "")).unwrap();
    doc.replace_paragraphs(
        &[(0, "第一段".into()), (1, "第二段".into())],
        &tracked(),
        "x",
    )
    .unwrap();
    assert_eq!(marked(&doc, 0), "[+第一段]");
    assert_eq!(marked(&doc, 1), "[+第二段]");
    assert_eq!(doc.paragraphs[1].align.as_deref(), Some("center"));
}

#[test]
fn undo_and_redo_restore_exact_bytes() {
    let original = testgen::generate(&Spec {
        target_chars: 20_000,
        comments: 30,
        seed: 3,
    });
    let mut doc = Document::from_bytes(original.clone()).unwrap();
    let target = (0..doc.paragraphs.len())
        .find(|&i| doc.heading_level(i).is_none() && doc.editable_text(i).chars().count() > 40)
        .unwrap();
    let old = doc.editable_text(target);
    let new = format!("经研究，{}", old.replacen('。', "；", 1));
    doc.replace_paragraphs(&[(target, new.clone())], &tracked(), "AI 修复")
        .unwrap();
    let edited = part(&doc, "word/document.xml");
    assert!(doc.is_dirty());
    assert_eq!(doc.undo_label(), Some("AI 修复"));

    assert_eq!(doc.undo().unwrap().as_deref(), Some("AI 修复"));
    assert_eq!(doc.editable_text(target), old);
    assert!(
        !doc.is_modified(),
        "undo returns every part to the original bytes"
    );
    assert!(!doc.is_dirty());
    assert_eq!(doc.redo_label(), Some("AI 修复"));

    doc.redo().unwrap();
    assert_eq!(doc.editable_text(target), new);
    assert_eq!(part(&doc, "word/document.xml"), edited);
    assert_eq!(doc.undo().unwrap().as_deref(), Some("AI 修复"));
    assert_eq!(doc.undo().unwrap(), None);
}

#[test]
fn edits_in_a_large_document_keep_everything_else() {
    let mut doc = Document::from_bytes(testgen::generate(&Spec::default())).unwrap();
    let comments_before = doc.comments.clone();
    let texts: Vec<String> = (0..doc.paragraphs.len())
        .map(|i| doc.editable_text(i))
        .collect();
    let targets: Vec<usize> = doc
        .comments
        .iter()
        .filter(|c| c.parent_id.is_none())
        .filter_map(|c| c.anchor.as_ref())
        .filter(|a| a.start_paragraph == a.end_paragraph)
        .map(|a| a.start_paragraph)
        .filter(|&p| {
            doc.paragraphs[p]
                .runs
                .iter()
                .all(|r| r.revision == Revision::None)
                && doc.editable_text(p).contains('。')
        })
        .take(20)
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    assert!(targets.len() >= 10);
    let changes: Vec<(usize, String)> = targets
        .iter()
        .map(|&p| (p, texts[p].replacen('。', "，并按要求落实。", 1)))
        .collect();

    let started = std::time::Instant::now();
    doc.replace_paragraphs(&changes, &tracked(), "批量修复")
        .unwrap();
    let elapsed = started.elapsed();
    assert!(elapsed.as_millis() < 2_000, "batch edit took {elapsed:?}");

    for (i, text) in texts.iter().enumerate() {
        let expected = changes
            .iter()
            .find(|(p, _)| *p == i)
            .map_or(text.as_str(), |(_, t)| t.as_str());
        assert_eq!(doc.editable_text(i), expected, "paragraph {i}");
    }
    assert_eq!(doc.comments.len(), comments_before.len());
    for (a, b) in doc.comments.iter().zip(&comments_before) {
        assert_eq!((&a.id, &a.author, a.done), (&b.id, &b.author, b.done));
        let (x, y) = (a.anchor.as_ref().unwrap(), b.anchor.as_ref().unwrap());
        assert_eq!(x.start_paragraph, y.start_paragraph);
        if !targets.contains(&x.start_paragraph) {
            assert_eq!(x.start_offset, y.start_offset, "comment {}", a.id);
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("out.docx");
    doc.save(&path).unwrap();
    assert!(!doc.is_dirty());
    let reopened = Document::open(&path).unwrap();
    assert_eq!(reopened.editable_text(targets[0]), changes[0].1);
}

#[test]
fn resolves_comments_and_creates_comments_extended() {
    let body = r#"<w:p><w:commentRangeStart w:id="1"/><w:r><w:t>正文</w:t></w:r><w:commentRangeEnd w:id="1"/><w:r><w:commentReference w:id="1"/></w:r></w:p>"#;
    let comments = r#"<w:comment w:id="1" w:author="专家A" w:initials="A"><w:p><w:r><w:t>意见</w:t></w:r></w:p></w:comment>"#;
    let mut doc = Document::from_bytes(package(body, comments)).unwrap();
    assert!(!has_part(&doc, "word/commentsExtended.xml"));

    doc.set_comments_done(&["1"], true).unwrap();
    assert!(doc.comments[0].done);
    assert!(part(&doc, "[Content_Types].xml").contains("/word/commentsExtended.xml"));
    assert!(part(&doc, "word/_rels/document.xml.rels").contains("commentsExtended.xml"));
    let comments_xml = part(&doc, "word/comments.xml");
    assert!(comments_xml.contains(W14));
    assert!(comments_xml.contains("w14:paraId=\""));
    assert!(reopen(&doc).comments[0].done);

    doc.set_comments_done(&["1"], false).unwrap();
    assert!(!doc.comments[0].done);
    doc.undo().unwrap();
    doc.undo().unwrap();
    assert!(!doc.is_modified());
    assert!(!has_part(&doc, "word/commentsExtended.xml"));
}

#[test]
fn renames_authors_and_adds_replies() {
    let mut doc = Document::from_bytes(testgen::generate(&Spec {
        target_chars: 5_000,
        comments: 8,
        seed: 11,
    }))
    .unwrap();
    let first = doc
        .comments
        .iter()
        .find(|c| c.parent_id.is_none() && !c.done)
        .unwrap()
        .clone();

    doc.set_comment_authors(&[&first.id], "张三（发改委）", Some("张"))
        .unwrap();
    let renamed = doc.comment(&first.id).unwrap();
    assert_eq!(renamed.author, "张三（发改委）");
    assert_eq!(renamed.initials.as_deref(), Some("张"));
    assert_eq!(renamed.text, first.text);

    let reply = doc
        .add_reply(&first.id, "AutoPassDoc", None, "已按意见修改。\n第二行")
        .unwrap();
    let r = doc.comment(&reply).unwrap().clone();
    assert_eq!(r.parent_id.as_deref(), Some(first.id.as_str()));
    assert_eq!(r.author, "AutoPassDoc");
    assert_eq!(r.text, "已按意见修改。\n第二行");
    assert_eq!(r.anchor, first.anchor);
    let summary = doc.summary();
    let view = summary.comments.iter().find(|c| c.id == reply).unwrap();
    assert_eq!(view.parent_id.as_deref(), Some(first.id.as_str()));

    doc.set_comments_done(&[&first.id], true).unwrap();
    assert!(doc.comment(&first.id).unwrap().done);
    assert!(
        doc.comment(&reply).unwrap().done,
        "resolving a thread resolves its replies"
    );

    let reopened = reopen(&doc);
    assert_eq!(reopened.comments.len(), doc.comments.len());
    assert_eq!(reopened.comment(&reply).unwrap().anchor, first.anchor);

    doc.undo().unwrap();
    doc.undo().unwrap();
    doc.undo().unwrap();
    assert!(!doc.is_modified());
}
