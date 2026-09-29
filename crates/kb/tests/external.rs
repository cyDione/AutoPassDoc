//! Markdown from online parsers, `import_parsed`, the document viewer and
//! folder expansion.

mod common;

use std::path::PathBuf;

use common::{SAMPLE, char_slice, kb_in, write};
use kb::{Error, SearchQuery, expand_import_paths, parse_external_markdown};

fn texts(md: &str) -> Vec<String> {
    parse_external_markdown(md)
        .into_iter()
        .map(|l| l.text)
        .filter(|t| !t.is_empty())
        .collect()
}

#[test]
fn html_tables_become_labelled_rows_with_merged_cells_repeated() {
    let md = r#"# 预算表

<table><tr><td rowspan="2">项目</td><td colspan="2">金额（万元）</td></tr>
<tr><td>2024年</td><td>2025年</td></tr>
<tr><td>硬件购置</td><td>1,260</td><td>800</td></tr>
<tr><td rowspan=2>软件</td><td>300</td><td>&lt;100</td></tr>
<tr><td>50</td><td>60</td></tr></table>

表后正文。"#;
    assert_eq!(
        texts(md),
        [
            "预算表",
            "表格列：项目 | 金额（万元）",
            "项目；金额（万元）：2024年；金额（万元）：2025年",
            "项目：硬件购置；金额（万元）：1,260；金额（万元）：800",
            "项目：软件；金额（万元）：300；金额（万元）：<100",
            "项目：软件；金额（万元）：50；金额（万元）：60",
            "表后正文。",
        ]
    );
}

#[test]
fn pipe_tables_and_small_tables() {
    let md = "说明如下：\n\n| 序号 | 名称 | 备注 |\n|:---|---|---:|\n| 1 | 路灯 | 含\\|杆 |\n| 2 | 信号灯 | |\n\n<table><tr><td>单独一格</td></tr></table>\n";
    assert_eq!(
        texts(md),
        [
            "说明如下：",
            "表格列：序号 | 名称 | 备注",
            "序号：1；名称：路灯；备注：含|杆",
            "序号：2；名称：信号灯",
            "单独一格",
        ]
    );
}

#[test]
fn drops_images_comments_and_tags() {
    let md = "![](images/a.jpg)\n<!-- page 1 -->\n正文&nbsp;第一段<sup>1</sup>，&#8220;引号&#x201D;。\n<!--\n多行注释\n-->\n![图 1](b.png)结尾";
    assert_eq!(texts(md), ["正文 第一段1，“引号”。", "结尾"]);
}

#[test]
fn numbered_headings_keep_their_ranks() {
    // MinerU marks every heading with a single `#`.
    let md = "# 某某市公共数据管理办法\n\n# 第一章 总则\n\n第一条 为了规范管理，制定本办法。\n\n# 第二章 目录\n\n# 第一节 目录管理\n\n第二条 目录由主管部门编制。";
    let levels: Vec<(String, Option<u8>)> = parse_external_markdown(md)
        .into_iter()
        .filter(|l| !l.text.is_empty())
        .map(|l| (l.text, l.style_level))
        .collect();
    assert_eq!(levels[0], ("某某市公共数据管理办法".to_string(), Some(0)));
    assert_eq!(levels[1], ("第一章 总则".to_string(), None));
    assert_eq!(levels[4], ("第一节 目录管理".to_string(), None));
}

#[test]
fn imports_parsed_text_and_views_it() {
    let dir = tempfile::tempdir().unwrap();
    let pdf = write(dir.path(), "扫描件.pdf", b"%PDF-1.4 not really");
    let mut kb = kb_in(dir.path());
    let md = format!("# 某某市公共数据管理办法\n\n{SAMPLE}");
    let report = kb.import_parsed(&pdf, &md, "mineru").unwrap();
    assert!(!report.unchanged && report.chunks > 10, "{report:?}");
    let doc = kb.document(report.doc_id).unwrap().unwrap();
    assert_eq!(doc.parser, "mineru");
    assert_eq!(doc.format, "pdf");
    assert_eq!(
        std::fs::read(&doc.stored_path).unwrap(),
        std::fs::read(&pdf).unwrap()
    );
    let hits = kb
        .search(&SearchQuery {
            text: "公共数据质量评价机制".into(),
            ..SearchQuery::default()
        })
        .unwrap();
    assert_eq!(hits[0].doc_id, report.doc_id);

    // Same parser, same content: nothing to do. The built-in parser never
    // replaces an online parser's text.
    assert!(
        kb.import_parsed(&pdf, "# 别的", "mineru")
            .unwrap()
            .unchanged
    );
    let again = kb.import_file(&pdf).unwrap();
    assert!(again.unchanged);
    assert_eq!(again.doc_id, report.doc_id);
    assert_eq!(
        kb.find_content(&pdf).unwrap().map(|d| d.parser).as_deref(),
        Some("mineru")
    );

    // Another parser replaces it in place.
    let other = kb
        .import_parsed(&pdf, "第一条 另一种解析结果。", "paddleocr")
        .unwrap();
    assert_eq!(other.doc_id, report.doc_id);
    assert!(!other.unchanged);
    assert_eq!(kb.documents().unwrap().len(), 1);
    assert_eq!(
        kb.document(other.doc_id).unwrap().unwrap().parser,
        "paddleocr"
    );

    let txt = write(dir.path(), "办法.txt", SAMPLE);
    let id = kb.import_file(&txt).unwrap().doc_id;
    let view = kb.document_view(id).unwrap();
    assert_eq!(view.document.parser, "builtin");
    let full = kb.full_text(id).unwrap().unwrap();
    assert_eq!(view.lines.len(), full.split('\n').count());
    let chapter = view
        .lines
        .iter()
        .find(|l| l.text.starts_with("第一章"))
        .unwrap();
    assert_eq!(chapter.heading_level, Some(2));
    assert_eq!(view.chunks.len(), view.document.chunk_count);
    for c in &view.chunks {
        let hit = kb.chunk(c.chunk_id).unwrap().unwrap();
        assert_eq!(char_slice(&full, c.char_start, c.char_end), hit.text);
        assert_eq!(c.heading_path, hit.heading_path);
    }
    assert!(
        view.chunks
            .windows(2)
            .all(|w| w[0].char_start <= w[1].char_start)
    );
    assert!(matches!(
        kb.document_view(9999),
        Err(Error::DocumentNotFound(9999))
    ));
}

#[test]
fn images_need_an_online_parser() {
    let dir = tempfile::tempdir().unwrap();
    let png = write(dir.path(), "照片.PNG", b"\x89PNG\r\n");
    let mut kb = kb_in(dir.path());
    let err = kb.import_file(&png).unwrap_err();
    assert!(matches!(err, Error::NeedsEnhanced));
    assert!(err.to_string().contains("图片需要增强解析"), "{err}");
    let report = kb
        .import_parsed(&png, "第一条 图片里的文字。", "paddleocr")
        .unwrap();
    assert_eq!(kb.document(report.doc_id).unwrap().unwrap().format, "image");
}

#[test]
fn expands_folders() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("资料");
    std::fs::create_dir_all(root.join("子目录/更深")).unwrap();
    std::fs::create_dir_all(root.join(".git")).unwrap();
    for name in [
        "b.docx",
        "a.PDF",
        "说明.txt",
        "图.png",
        "表.xlsx",
        "~$b.docx",
        ".hidden.md",
        ".git/x.md",
        "子目录/c.md",
        "子目录/更深/d.markdown",
    ] {
        write(&root, name, "x");
    }
    let names = |paths: Vec<PathBuf>| -> Vec<String> {
        paths
            .iter()
            .map(|p| {
                p.strip_prefix(&root)
                    .unwrap_or(p)
                    .to_string_lossy()
                    .replace('\\', "/")
            })
            .collect()
    };
    let loose = write(dir.path(), "单独.xlsx", "x");
    let found = expand_import_paths(&[root.clone(), loose.clone(), root.clone()], false);
    assert_eq!(found.last(), Some(&loose), "explicit files are kept");
    assert_eq!(
        names(found[..found.len() - 1].to_vec()),
        [
            "a.PDF",
            "b.docx",
            "子目录/c.md",
            "子目录/更深/d.markdown",
            "说明.txt"
        ]
    );
    let with_images = names(expand_import_paths(std::slice::from_ref(&root), true));
    assert!(with_images.contains(&"图.png".to_string()));
    assert_eq!(with_images.len(), 6);

    #[cfg(unix)]
    {
        let outside = tempfile::tempdir().unwrap();
        write(outside.path(), "外部.md", "x");
        std::os::unix::fs::symlink(outside.path(), root.join("链接")).unwrap();
        assert_eq!(
            expand_import_paths(std::slice::from_ref(&root), false).len(),
            5,
            "symbolic links are not followed"
        );
    }
}

#[test]
fn upgrades_a_v1_database() {
    let dir = tempfile::tempdir().unwrap();
    let txt = write(dir.path(), "办法.txt", SAMPLE);
    let id = kb_in(dir.path()).import_file(&txt).unwrap().doc_id;
    {
        let conn = rusqlite::Connection::open(dir.path().join("kb/kb.sqlite")).unwrap();
        conn.execute_batch("ALTER TABLE documents DROP COLUMN parser; PRAGMA user_version = 1;")
            .unwrap();
    }
    let kb = kb_in(dir.path());
    assert_eq!(kb.document(id).unwrap().unwrap().parser, "builtin");
}
