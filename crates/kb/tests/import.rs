mod common;

use std::collections::BTreeSet;
use std::io::Write;

use common::{SAMPLE, char_slice, kb_in, write};
use kb::{DocMeta, Error, KnowledgeBase, SearchQuery};

fn query(text: &str) -> SearchQuery {
    SearchQuery {
        text: text.into(),
        ..SearchQuery::default()
    }
}

/// Checks that every chunk's offsets point at its text in the stored full text.
fn assert_offsets(kb: &KnowledgeBase, doc_id: i64) {
    let full = kb.full_text(doc_id).unwrap().unwrap();
    let (_, total) = kb.embedding_progress("none").unwrap();
    for (id, _) in kb.pending_embeddings("none", total).unwrap() {
        let hit = kb.chunk(id).unwrap().unwrap();
        if hit.doc_id == doc_id {
            assert_eq!(char_slice(&full, hit.char_start, hit.char_end), hit.text);
            assert!(hit.parent_text.contains(&hit.text));
        }
    }
}

#[test]
fn imports_utf8_text_with_metadata_and_a_stored_copy() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), "数据管理办法.txt", SAMPLE.replace('\n', "\r\n"));
    let mut kb = kb_in(dir.path());
    let report = kb.import_file(&path).unwrap();
    assert!(!report.unchanged);
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert!(report.chunks >= 15);

    let doc = kb.document(report.doc_id).unwrap().unwrap();
    assert_eq!(doc.format, "txt");
    assert_eq!(doc.file_name, "数据管理办法.txt");
    assert_eq!(doc.chunk_count, report.chunks);
    assert_eq!(
        doc.title,
        "某某市人民政府关于印发《某某市公共数据管理办法》的通知"
    );
    assert_eq!(doc.meta.doc_number.as_deref(), Some("某政发〔2024〕7号"));
    assert_eq!(doc.meta.issuer.as_deref(), Some("某某市人民政府"));
    assert_eq!(doc.meta.date.as_deref(), Some("2024-03-15"));
    assert_eq!(doc.sha256.len(), 64);
    assert!(
        doc.stored_path
            .starts_with(dir.path().join("kb").join("files"))
    );
    assert_eq!(
        std::fs::read(&doc.stored_path).unwrap(),
        std::fs::read(&path).unwrap()
    );
    assert_eq!(doc.original_path, std::path::absolute(&path).unwrap());
    // CRLF is normalised: the full text is the sample itself.
    assert_eq!(kb.full_text(doc.id).unwrap().unwrap(), SAMPLE);
    assert_eq!(
        doc.char_count,
        SAMPLE.chars().filter(|c| !c.is_whitespace()).count()
    );
    assert_offsets(&kb, doc.id);
}

#[test]
fn imports_gbk_text() {
    let dir = tempfile::tempdir().unwrap();
    let (gbk, _, unmappable) = encoding_rs::GBK.encode(SAMPLE);
    assert!(!unmappable);
    assert!(std::str::from_utf8(&gbk).is_err());
    let path = write(dir.path(), "gbk.txt", &gbk);
    let mut kb = kb_in(dir.path());
    let report = kb.import_file(&path).unwrap();
    assert_eq!(kb.full_text(report.doc_id).unwrap().unwrap(), SAMPLE);
    let hits = kb.search(&query("绩效考核")).unwrap();
    assert_eq!(hits[0].heading_path, ["第三章 监督管理", "第二十条"]);
}

#[test]
fn imports_markdown_with_heading_levels() {
    let dir = tempfile::tempdir().unwrap();
    let body = "应当建立健全数据安全管理制度，落实数据安全保护责任。".repeat(12);
    let md = format!(
        "# 数据安全管理规范\n\n## 第一章 总则\n\n{body}\n\n## 第二章 分类分级\n\n### 重要数据识别\n\n重要数据目录由主管部门统一发布。{body}\n\n```\n# 这不是标题\n```\n"
    );
    let path = write(dir.path(), "规范.md", &md);
    let mut kb = kb_in(dir.path());
    let report = kb.import_file(&path).unwrap();
    let doc = kb.document(report.doc_id).unwrap().unwrap();
    assert_eq!(doc.format, "md");
    assert_eq!(doc.title, "数据安全管理规范");
    let hits = kb.search(&query("重要数据目录")).unwrap();
    assert_eq!(
        hits[0].heading_path,
        ["数据安全管理规范", "第二章 分类分级", "重要数据识别"]
    );
    assert!(
        hits.iter()
            .all(|h| !h.heading_path.iter().any(|p| p.contains("不是标题")))
    );
    assert!(hits.iter().all(|h| !h.text.contains("##")));
    assert_offsets(&kb, doc.id);
}

const DOCX_STYLES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style><w:style w:type="paragraph" w:styleId="a3"><w:name w:val="Title"/><w:basedOn w:val="Normal"/></w:style><w:style w:type="paragraph" w:styleId="1"><w:name w:val="heading 1"/><w:basedOn w:val="Normal"/><w:pPr><w:outlineLvl w:val="0"/></w:pPr></w:style></w:styles>"#;

fn docx(paragraphs: &str) -> Vec<u8> {
    let document = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body>{paragraphs}</w:body></w:document>"#
    );
    let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    for (name, data) in [
        (
            "[Content_Types].xml",
            r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#,
        ),
        (
            "_rels/.rels",
            r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#,
        ),
        ("word/document.xml", &document),
        ("word/styles.xml", DOCX_STYLES),
    ] {
        zip.start_file(name, options).unwrap();
        zip.write_all(data.as_bytes()).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

fn p(style: Option<&str>, runs: &str) -> String {
    let ppr = style.map_or(String::new(), |s| {
        format!(r#"<w:pPr><w:pStyle w:val="{s}"/></w:pPr>"#)
    });
    format!("<w:p>{ppr}{runs}</w:p>")
}

fn r(text: &str) -> String {
    format!(r#"<w:r><w:t xml:space="preserve">{text}</w:t></w:r>"#)
}

#[test]
fn imports_docx_with_styles_revisions_and_tables() {
    let dir = tempfile::tempdir().unwrap();
    let filler = "数据处理者应当依照法律法规的规定，建立健全全流程数据安全管理制度，组织开展数据安全教育培训。".repeat(10);
    let body = [
        p(Some("a3"), &r("某某市数据安全管理办法")),
        p(None, &r("某数发〔2024〕9号")),
        p(Some("1"), &r("第一章 总则")),
        p(
            None,
            &format!(
                r#"{}<w:del w:id="1" w:author="审稿人"><w:r><w:delText>（草案）</w:delText></w:r></w:del>{}"#,
                r("第一条 为了加强数据安全管理"),
                r("，制定本办法。")
            ),
        ),
        p(None, &format!("{}<w:r><w:br/></w:r>{}", r("第二条 本办法适用于本市行政区域内的数据处理活动。"), r(&filler))),
        format!(
            "<w:tbl><w:tr><w:tc>{}</w:tc><w:tc>{}</w:tc></w:tr></w:tbl>",
            p(None, &r("数据类别")),
            p(None, &r("核心数据实行更加严格的管理制度"))
        ),
        p(Some("1"), &r("第二章 监督检查")),
        p(None, &r(&format!("第三条 有关部门应当加强监督检查。{filler}"))),
        p(None, &r("某某市数据局")),
        p(None, &r("2024年6月1日")),
    ]
    .concat();
    let path = write(dir.path(), "办法.docx", docx(&body));
    let mut kb = kb_in(dir.path());
    let report = kb.import_file(&path).unwrap();
    let doc = kb.document(report.doc_id).unwrap().unwrap();
    assert_eq!(doc.format, "docx");
    assert_eq!(doc.title, "某某市数据安全管理办法");
    assert_eq!(doc.meta.doc_number.as_deref(), Some("某数发〔2024〕9号"));
    assert_eq!(doc.meta.issuer.as_deref(), Some("某某市数据局"));
    assert_eq!(doc.meta.date.as_deref(), Some("2024-06-01"));

    // A paragraph is one line and a table row is one line; deleted text is
    // gone and soft breaks became spaces.
    let full = kb.full_text(doc.id).unwrap().unwrap();
    let lines: Vec<&str> = full.split('\n').collect();
    assert_eq!(lines.len(), 10);
    assert_eq!(lines[3], "第一条 为了加强数据安全管理，制定本办法。");
    assert!(lines[4].starts_with("第二条 本办法适用于本市行政区域内的数据处理活动。 数据处理者"));
    assert_eq!(lines[5], "数据类别 | 核心数据实行更加严格的管理制度");

    let hits = kb.search(&query("第一条")).unwrap();
    assert_eq!(hits[0].heading_path, ["第一章 总则", "第一条"]);
    let hits = kb.search(&query("核心数据")).unwrap();
    assert!(hits[0].text.contains("核心数据实行更加严格的管理制度"));
    assert_offsets(&kb, doc.id);
}

fn tc(text: &str, props: &str) -> String {
    format!("<w:tc><w:tcPr>{props}</w:tcPr>{}</w:tc>", p(None, &r(text)))
}

#[test]
fn tables_keep_column_headers_on_every_row() {
    let dir = tempfile::tempdir().unwrap();
    let row = |cells: &[String]| format!("<w:tr>{}</w:tr>", cells.concat());
    let table = format!(
        "<w:tbl>{}{}{}{}</w:tbl>",
        row(&[tc("类别", ""), tc("项目", ""), tc("金额（万元）", "")]),
        row(&[
            tc("建设投资", r#"<w:vMerge w:val="restart"/>"#),
            tc("硬件设备购置", ""),
            tc("1260", "")
        ]),
        row(&[tc("", "<w:vMerge/>"), tc("软件开发", ""), tc("1720", "")]),
        row(&[tc("合计", r#"<w:gridSpan w:val="2"/>"#), tc("3850", "")]),
    );
    let body = [
        p(Some("1"), &r("第一章 投资估算")),
        p(None, &r("表1 项目投资估算表")),
        table,
    ]
    .concat();
    let path = write(dir.path(), "估算.docx", docx(&body));
    let mut kb = kb_in(dir.path());
    let report = kb.import_file(&path).unwrap();
    let full = kb.full_text(report.doc_id).unwrap().unwrap();
    let lines: Vec<&str> = full.split('\n').collect();
    assert_eq!(
        lines[2..],
        [
            "表格列：类别 | 项目 | 金额（万元）",
            "类别：建设投资；项目：硬件设备购置；金额（万元）：1260",
            "类别：建设投资；项目：软件开发；金额（万元）：1720",
            "类别：合计；项目：合计；金额（万元）：3850",
        ]
    );
    let hits = kb.search(&query("软件开发金额")).unwrap();
    assert!(hits[0].text.contains("项目：软件开发；金额（万元）：1720"));
    assert_offsets(&kb, report.doc_id);
}

#[test]
fn imports_generated_report_docx() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = docx_engine::testgen::generate(&docx_engine::testgen::Spec {
        target_chars: 6_000,
        comments: 5,
        seed: 3,
    });
    let path = write(dir.path(), "报告.docx", bytes);
    let mut kb = kb_in(dir.path());
    let report = kb.import_file(&path).unwrap();
    assert!(report.chunks > 10);
    let hits = kb.search(&query("数据中台 业务应用系统")).unwrap();
    assert!(!hits.is_empty());
    // Heading styles with automatic numbering give the path.
    let path = &hits[0].heading_path;
    assert_eq!(path.len(), 3, "{path:?}");
    assert!(path[0].starts_with("一、第1部分"), "{path:?}");
    assert!(path[1].starts_with("（"), "{path:?}");
    assert_offsets(&kb, report.doc_id);
}

mod pdf {
    use super::*;
    use pdf_extract::content::{Content, Operation};
    use pdf_extract::{Dictionary, Document, Object, Stream, StringFormat};

    fn dict(entries: Vec<(&str, Object)>) -> Dictionary {
        let mut d = Dictionary::new();
        for (k, v) in entries {
            d.set(k, v);
        }
        d
    }

    /// A PDF with one line of text per entry, using a CID font with a
    /// ToUnicode map, like PDFs exported from Word.
    pub fn make(pages: &[&[&str]]) -> Vec<u8> {
        let chars: BTreeSet<char> = pages
            .iter()
            .flat_map(|p| p.iter())
            .flat_map(|l| l.chars())
            .collect();
        let mut cmap = String::from(
            "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n",
        );
        let chars: Vec<char> = chars.into_iter().collect();
        for block in chars.chunks(100) {
            cmap.push_str(&format!("{} beginbfchar\n", block.len()));
            for c in block {
                cmap.push_str(&format!("<{:04X}> <{:04X}>\n", *c as u32, *c as u32));
            }
            cmap.push_str("endbfchar\n");
        }
        cmap.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");

        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let to_unicode = doc.add_object(Stream::new(Dictionary::new(), cmap.into_bytes()));
        let descriptor = doc.add_object(dict(vec![
            ("Type", "FontDescriptor".into()),
            ("FontName", Object::Name(b"STSong-Light".to_vec())),
            ("Flags", 6.into()),
            (
                "FontBBox",
                vec![0.into(), (-200).into(), 1000.into(), 900.into()].into(),
            ),
            ("ItalicAngle", 0.into()),
            ("Ascent", 880.into()),
            ("Descent", (-120).into()),
            ("CapHeight", 880.into()),
            ("StemV", 80.into()),
        ]));
        let cid_font = doc.add_object(dict(vec![
            ("Type", Object::Name(b"Font".to_vec())),
            ("Subtype", Object::Name(b"CIDFontType0".to_vec())),
            ("BaseFont", Object::Name(b"STSong-Light".to_vec())),
            (
                "CIDSystemInfo",
                dict(vec![
                    ("Registry", Object::string_literal("Adobe")),
                    ("Ordering", Object::string_literal("Identity")),
                    ("Supplement", 0.into()),
                ])
                .into(),
            ),
            ("FontDescriptor", descriptor.into()),
            ("DW", 1000.into()),
        ]));
        let font = doc.add_object(dict(vec![
            ("Type", Object::Name(b"Font".to_vec())),
            ("Subtype", Object::Name(b"Type0".to_vec())),
            ("BaseFont", Object::Name(b"STSong-Light".to_vec())),
            ("Encoding", Object::Name(b"Identity-H".to_vec())),
            ("DescendantFonts", vec![cid_font.into()].into()),
            ("ToUnicode", to_unicode.into()),
        ]));
        let resources =
            doc.add_object(dict(vec![("Font", dict(vec![("F1", font.into())]).into())]));
        let mut kids = Vec::new();
        for lines in pages {
            let mut operations = Vec::new();
            for (i, line) in lines.iter().enumerate() {
                let code: Vec<u8> = line
                    .chars()
                    .flat_map(|c| (c as u16).to_be_bytes())
                    .collect();
                operations.extend([
                    Operation::new("BT", vec![]),
                    Operation::new("Tf", vec![Object::Name(b"F1".to_vec()), 12.into()]),
                    Operation::new("Td", vec![50.into(), (800 - 20 * i as i64).into()]),
                    Operation::new("Tj", vec![Object::String(code, StringFormat::Hexadecimal)]),
                    Operation::new("ET", vec![]),
                ]);
            }
            let content = Content { operations }.encode().unwrap();
            let content_id = doc.add_object(Stream::new(Dictionary::new(), content));
            kids.push(
                doc.add_object(dict(vec![
                    ("Type", Object::Name(b"Page".to_vec())),
                    ("Parent", pages_id.into()),
                    ("Contents", content_id.into()),
                    ("Resources", resources.into()),
                    (
                        "MediaBox",
                        vec![0.into(), 0.into(), 595.into(), 842.into()].into(),
                    ),
                ]))
                .into(),
            );
        }
        let count = kids.len() as i64;
        doc.objects.insert(
            pages_id,
            dict(vec![
                ("Type", Object::Name(b"Pages".to_vec())),
                ("Kids", kids.into()),
                ("Count", count.into()),
            ])
            .into(),
        );
        let catalog = doc.add_object(dict(vec![
            ("Type", Object::Name(b"Catalog".to_vec())),
            ("Pages", pages_id.into()),
        ]));
        doc.trailer.set("Root", catalog);
        let mut out = Vec::new();
        doc.save_to(&mut out).unwrap();
        out
    }
}

/// Lines of at most `width` chars, as a PDF lays out a paragraph.
fn wrap(paragraph: &str, width: usize) -> Vec<String> {
    let chars: Vec<char> = paragraph.chars().collect();
    chars.chunks(width).map(|c| c.iter().collect()).collect()
}

#[test]
fn imports_text_pdf_and_rejoins_wrapped_lines() {
    let dir = tempfile::tempdir().unwrap();
    let article1 = "第一条 为了促进公共数据开放和利用，提升政府治理能力和公共服务水平，根据有关法律法规，结合本市实际，制定本规定。";
    let article2 =
        "第二条 公共数据开放应当遵循需求导向、安全可控、分类分级、统一标准的原则，依法有序开放。";
    let article3 = "第三条 市数据主管部门负责统筹全市公共数据开放工作，区数据主管部门负责本区公共数据开放工作。";
    let mut page1 = vec![
        "某某市公共数据开放管理规定".to_string(),
        "第一章 总则".to_string(),
    ];
    page1.extend(wrap(article1, 24));
    page1.push("- 1 -".to_string());
    let mut page2 = wrap(article2, 24);
    page2.extend(wrap(article3, 24));
    page2.extend(["某某市人民政府".to_string(), "2023年12月5日".to_string()]);
    let page1: Vec<&str> = page1.iter().map(String::as_str).collect();
    let page2: Vec<&str> = page2.iter().map(String::as_str).collect();
    let path = write(dir.path(), "开放规定.pdf", pdf::make(&[&page1, &page2]));

    let mut kb = kb_in(dir.path());
    let report = kb.import_file(&path).unwrap();
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    let doc = kb.document(report.doc_id).unwrap().unwrap();
    assert_eq!(doc.format, "pdf");
    assert_eq!(doc.title, "某某市公共数据开放管理规定");
    assert_eq!(doc.meta.issuer.as_deref(), Some("某某市人民政府"));
    assert_eq!(doc.meta.date.as_deref(), Some("2023-12-05"));
    let full = kb.full_text(doc.id).unwrap().unwrap();
    let lines: Vec<&str> = full.lines().collect();
    assert_eq!(
        lines,
        [
            "某某市公共数据开放管理规定",
            "第一章 总则",
            article1,
            article2,
            article3,
            "某某市人民政府",
            "2023年12月5日"
        ]
    );
    let hits = kb.search(&query("第二条")).unwrap();
    assert_eq!(hits[0].heading_path, ["第一章 总则", "第二条"]);
    assert_eq!(hits[0].text, article2);
    assert_offsets(&kb, doc.id);
}

#[test]
fn scanned_pdf_imports_without_chunks_and_warns() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), "扫描件.pdf", pdf::make(&[&[], &["第1页"]]));
    let mut kb = kb_in(dir.path());
    let report = kb.import_file(&path).unwrap();
    assert_eq!(report.chunks, 0);
    assert_eq!(
        report.warnings,
        ["扫描版 PDF 没有文字层，需要 OCR 后再导入"]
    );
    let doc = kb.document(report.doc_id).unwrap().unwrap();
    assert_eq!(doc.chunk_count, 0);
    assert_eq!(doc.warnings, report.warnings);
}

#[test]
fn rejects_unsupported_and_broken_files() {
    let dir = tempfile::tempdir().unwrap();
    let mut kb = kb_in(dir.path());
    let xls = write(dir.path(), "表格.xls", b"not really");
    let err = kb.import_file(&xls).unwrap_err();
    assert!(matches!(err, Error::UnsupportedFormat(_)));
    assert!(err.to_string().starts_with("不支持的格式"), "{err}");
    let broken = write(dir.path(), "坏文件.docx", b"PK broken");
    assert!(matches!(kb.import_file(&broken), Err(Error::Parse(_))));
    let missing = dir.path().join("不存在.txt");
    assert!(matches!(kb.import_file(&missing), Err(Error::Read { .. })));
    assert!(kb.documents().unwrap().is_empty());
}

#[test]
fn reimport_skips_unchanged_and_replaces_changed_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), "办法.txt", SAMPLE);
    let mut kb = kb_in(dir.path());
    let first = kb.import_file(&path).unwrap();

    let again = kb.import_file(&path).unwrap();
    assert!(again.unchanged);
    assert_eq!(again.doc_id, first.doc_id);
    assert_eq!(again.chunks, first.chunks);
    // A copy with identical content elsewhere is the same document too.
    let copy = write(dir.path(), "副本.txt", SAMPLE);
    assert!(kb.import_file(&copy).unwrap().unchanged);
    assert_eq!(kb.documents().unwrap().len(), 1);
    assert_eq!(kb.stats().unwrap().chunks, first.chunks);

    // The user corrects the metadata, embeds, then the file changes on disk.
    let corrected = DocMeta {
        issuer: Some("某某市人民政府办公厅".into()),
        ..kb.document(first.doc_id).unwrap().unwrap().meta
    };
    kb.update_metadata(first.doc_id, &corrected).unwrap();
    let pending = kb.pending_embeddings("m", 1000).unwrap();
    let vectors: Vec<(i64, Vec<f32>)> = pending
        .iter()
        .map(|(id, _)| (*id, vec![1.0, 0.0]))
        .collect();
    kb.store_embeddings("m", &vectors).unwrap();

    let changed = SAMPLE.replace("三个工作日", "两个工作日");
    std::fs::write(&path, &changed).unwrap();
    let replaced = kb.import_file(&path).unwrap();
    assert!(!replaced.unchanged);
    assert_eq!(replaced.doc_id, first.doc_id);
    let docs = kb.documents().unwrap();
    assert_eq!(docs.len(), 1);
    assert_eq!(docs[0].meta, corrected);
    assert_eq!(
        std::fs::read_to_string(&docs[0].stored_path).unwrap(),
        changed
    );
    assert_eq!(kb.stats().unwrap().chunks, replaced.chunks);
    assert_eq!(kb.embedding_progress("m").unwrap(), (0, replaced.chunks));
    assert!(
        kb.search(&query("两个工作日")).unwrap()[0]
            .text
            .contains("两个工作日")
    );
}

#[test]
fn update_metadata_normalizes_and_is_searchable() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(
        dir.path(),
        "无文号.txt",
        "关于加强值班工作的通知\n各单位要做好值班安排。",
    );
    let mut kb = kb_in(dir.path());
    let id = kb.import_file(&path).unwrap().doc_id;
    kb.update_metadata(
        id,
        &DocMeta {
            title: Some("  值班通知  ".into()),
            doc_number: Some("某办发[2024]15号".into()),
            issuer: Some("".into()),
            date: Some("2024-2-9".into()),
        },
    )
    .unwrap();
    let doc = kb.document(id).unwrap().unwrap();
    assert_eq!(doc.title, "值班通知");
    assert_eq!(doc.meta.doc_number.as_deref(), Some("某办发〔2024〕15号"));
    assert_eq!(doc.meta.issuer, None);
    assert_eq!(doc.meta.date.as_deref(), Some("2024-02-09"));
    let hits = kb.search(&query("某办发〔2024〕15号")).unwrap();
    assert_eq!(hits[0].doc_id, id);
    assert_eq!(hits[0].title, "值班通知");
    assert!(matches!(
        kb.update_metadata(999, &DocMeta::default()),
        Err(Error::DocumentNotFound(999))
    ));
}

#[test]
fn reopening_keeps_everything() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), "办法.txt", SAMPLE);
    let id = {
        let mut kb = kb_in(dir.path());
        kb.import_file(&path).unwrap().doc_id
    };
    let kb = kb_in(dir.path());
    assert_eq!(kb.documents().unwrap()[0].id, id);
    assert!(!kb.search(&query("公共数据")).unwrap().is_empty());
}
