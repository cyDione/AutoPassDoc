mod common;

use common::{SAMPLE, char_slice};
use kb::chunking::{
    CHILD_MAX_CHARS, HeadingKind, SourceLine, chunk_lines, chunk_text, detect_heading,
};

fn kind(line: &str) -> Option<HeadingKind> {
    detect_heading(line).map(|h| h.kind)
}

#[test]
fn detects_official_document_headings() {
    assert_eq!(kind("第一编　总则"), Some(HeadingKind::Part));
    assert_eq!(kind("第二部分 主要任务"), Some(HeadingKind::Part));
    assert_eq!(kind("第三章　监督管理"), Some(HeadingKind::Chapter));
    assert_eq!(kind("第12章 附则"), Some(HeadingKind::Chapter));
    assert_eq!(kind("第一节 目录管理"), Some(HeadingKind::Section));
    assert_eq!(kind("一、总体要求"), Some(HeadingKind::Outline1));
    assert_eq!(kind("（一）基本原则"), Some(HeadingKind::Outline2));
    assert_eq!(kind("(二) 适用范围"), Some(HeadingKind::Outline2));
    assert_eq!(kind("1. 分类原则"), Some(HeadingKind::Outline3));
    assert_eq!(kind("2．分级原则"), Some(HeadingKind::Outline3));
    assert_eq!(kind("3、工作要求"), Some(HeadingKind::Outline3));
    assert_eq!(kind("（1）人口数据"), Some(HeadingKind::Outline4));
    assert_eq!(kind("①户籍信息"), Some(HeadingKind::Outline5));
    assert_eq!(kind("附件2"), Some(HeadingKind::Attachment));

    // An article: the label is the heading, the rest of the line is body.
    let article = detect_heading("第二十条　县级以上人民政府应当加强监督。").unwrap();
    assert_eq!(article.kind, HeadingKind::Article);
    assert_eq!(article.text, "第二十条");
    assert!(article.has_body);
    let titled = detect_heading("第一条【立法目的】为了规范公共数据管理，制定本办法。").unwrap();
    assert_eq!(titled.text, "第一条【立法目的】");
    // Letter-spaced chapter titles are closed up.
    assert_eq!(
        detect_heading("第一章　总　则").unwrap().text,
        "第一章 总则"
    );
    // A run-in heading keeps its lead phrase.
    let lead = detect_heading(&format!(
        "（一）加强组织领导。{}",
        "各地区各部门要高度重视。".repeat(8)
    ))
    .unwrap();
    assert_eq!(lead.text, "（一）加强组织领导");
    assert!(lead.has_body);

    // Not headings: citations, long list items, numbers, enumerations.
    assert_eq!(kind("第二十条规定的情形除外。"), None);
    assert_eq!(kind("第五条第二款所列数据，应当予以公开。"), None);
    assert_eq!(
        kind(&format!(
            "1. {}",
            "各级人民政府应当加强公共数据管理工作".repeat(4)
        )),
        None
    );
    assert_eq!(kind("1.5亿元用于平台建设"), None);
    assert_eq!(kind("（一）涉及国家秘密的数据；"), None);
    assert_eq!(kind("2024年3月15日"), None);
    assert_eq!(kind(&format!("第三章{}", "很长的正文".repeat(10))), None);
}

#[test]
fn chunks_follow_structure_and_offsets_match_full_text() {
    let chunked = chunk_text(SAMPLE);
    assert_eq!(chunked.full_text, SAMPLE);
    let children: Vec<_> = chunked.parents.iter().flat_map(|p| &p.children).collect();

    for p in &chunked.parents {
        assert_eq!(
            char_slice(&chunked.full_text, p.char_start, p.char_end),
            p.text
        );
        assert!(p.text.chars().count() <= kb::chunking::PARENT_MAX_CHARS);
    }
    for c in &children {
        assert_eq!(
            char_slice(&chunked.full_text, c.char_start, c.char_end),
            c.text
        );
        assert!(c.text.chars().count() <= CHILD_MAX_CHARS, "{}", c.text);
        assert_eq!(c.text.trim(), c.text);
    }
    // Nothing is lost: every non-whitespace char is inside some child.
    let mut covered = vec![false; SAMPLE.chars().count()];
    for c in &children {
        covered[c.char_start..c.char_end].fill(true);
    }
    for (i, ch) in SAMPLE.chars().enumerate() {
        assert!(
            ch.is_whitespace() || covered[i],
            "char {i} {ch:?} is in no chunk"
        );
    }

    let path_of = |needle: &str| {
        children
            .iter()
            .find(|c| c.text.contains(needle))
            .unwrap_or_else(|| panic!("no chunk contains {needle}"))
            .heading_path
            .clone()
    };
    // The notice before the first heading has no path.
    assert!(path_of("某政发〔2024〕7号").is_empty());
    assert_eq!(path_of("纳入绩效考核体系"), ["第三章 监督管理", "第二十条"]);
    assert_eq!(
        path_of("本办法自2024年5月1日起施行"),
        ["第四章 附则", "第二十二条"]
    );
    assert_eq!(
        path_of("目录编制规范"),
        ["第二章 目录与归集", "第一节 目录管理", "第四条"]
    );
    // 款/项 items stay inside their article.
    assert_eq!(
        path_of("涉及商业秘密"),
        ["第二章 目录与归集", "第一节 目录管理", "第五条"]
    );
    assert_eq!(
        path_of("科学性、稳定性"),
        ["附件", "一、总体要求", "（一）基本原则", "1. 分类原则"]
    );
    assert_eq!(
        path_of("其他单位可以参照执行"),
        ["附件", "一、总体要求", "（二）适用范围"]
    );
    assert_eq!(path_of("空间地理"), ["附件", "二、分类方法"]);

    // A chapter heading joins the segment of its first article.
    let article_one = children.iter().find(|c| c.text.contains("第一条")).unwrap();
    assert!(article_one.text.starts_with("第一章　总　则\n第一条"));

    // The long article is one parent split into children at sentence ends.
    let long: Vec<_> = chunked
        .parents
        .iter()
        .filter(|p| p.heading_path.last().is_some_and(|h| h == "第六条"))
        .collect();
    assert_eq!(long.len(), 1);
    let parent = long[0];
    assert!(parent.text.chars().count() > CHILD_MAX_CHARS);
    assert!(parent.children.len() >= 2);
    for c in &parent.children {
        assert!(c.text.ends_with('。'), "child cut mid-sentence: {}", c.text);
        assert_eq!(
            c.heading_path,
            ["第二章 目录与归集", "第二节 数据归集", "第六条"]
        );
    }
    assert!(
        parent.children[0]
            .text
            .starts_with("第二节　数据归集\n第六条")
    );
}

#[test]
fn very_long_sentences_and_lines_are_still_bounded() {
    let sentence = "公共数据".repeat(150);
    let text = format!("{sentence}，{sentence}。\n{}", "短句。".repeat(700));
    let chunked = chunk_text(&text);
    assert!(chunked.parents.len() >= 2);
    for c in chunked.parents.iter().flat_map(|p| &p.children) {
        assert!(c.text.chars().count() <= CHILD_MAX_CHARS);
        assert_eq!(
            char_slice(&chunked.full_text, c.char_start, c.char_end),
            c.text
        );
    }
}

#[test]
fn short_documents_are_one_parent() {
    let text = "关于做好节假日值班工作的通知\n一、加强值班值守\n各单位要严格落实领导带班制度。\n二、做好应急准备\n遇有突发事件及时报告。";
    let chunked = chunk_text(text);
    assert_eq!(chunked.parents.len(), 1);
    assert_eq!(chunked.parents[0].text, text);
    let paths: Vec<_> = chunked.parents[0]
        .children
        .iter()
        .map(|c| c.heading_path.clone())
        .collect();
    assert_eq!(
        paths,
        [vec![], vec!["一、加强值班值守"], vec!["二、做好应急准备"]]
    );
}

#[test]
fn style_levels_rank_headings_without_numbering() {
    let styled = |text: &str, level| SourceLine {
        text: text.into(),
        style_level: Some(level),
        display: None,
    };
    let body = "正文内容。".repeat(120);
    let lines = vec![
        styled("项目概述", 0),
        styled("建设背景", 1),
        SourceLine::new("一、现状分析"),
        SourceLine::new(body.clone()),
        styled("建设目标", 1),
        SourceLine::new(body.clone()),
        styled("实施计划", 0),
        SourceLine::new(body),
    ];
    let chunked = chunk_lines(&lines);
    let paths: Vec<_> = chunked
        .parents
        .iter()
        .map(|p| p.heading_path.clone())
        .collect();
    assert_eq!(
        paths,
        [
            vec!["项目概述", "建设背景", "一、现状分析"],
            vec!["项目概述", "建设目标"],
            vec!["实施计划"],
        ]
    );
}
