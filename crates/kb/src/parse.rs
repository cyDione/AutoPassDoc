//! Reading supported file formats into lines.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

use crate::chunking::{SourceLine, detect_heading};
use crate::text::is_han;
use crate::{Error, Result};

pub(crate) const SCANNED_PDF_WARNING: &str = "扫描版 PDF 没有文字层，需要 OCR 后再导入";
/// A PDF with fewer non-whitespace chars per page than this is treated as scanned.
const MIN_PDF_CHARS_PER_PAGE: usize = 20;
/// Narrowest text width, in chars, at which PDF lines are taken to be wrapped.
const MIN_WRAP_WIDTH: usize = 15;

pub(crate) struct ParsedFile {
    pub lines: Vec<SourceLine>,
    /// Title from markup: a Word paragraph in the Title style, or a leading Markdown `#`.
    pub title_hint: Option<String>,
    pub warnings: Vec<String>,
    /// Text too sparse to index (scanned PDF).
    pub no_text: bool,
}

impl ParsedFile {
    fn new(lines: Vec<SourceLine>) -> Self {
        Self {
            lines,
            title_hint: None,
            warnings: Vec::new(),
            no_text: false,
        }
    }
}

/// Format name for a path, from its extension.
pub(crate) fn format_of(path: &Path) -> Result<&'static str> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    match ext.as_str() {
        "docx" => Ok("docx"),
        "pdf" => Ok("pdf"),
        "txt" | "text" => Ok("txt"),
        "md" | "markdown" => Ok("md"),
        "" => Err(Error::UnsupportedFormat("无扩展名".into())),
        other => Err(Error::UnsupportedFormat(format!(".{other}"))),
    }
}

pub(crate) fn parse(format: &str, bytes: &[u8]) -> Result<ParsedFile> {
    match format {
        "docx" => parse_docx(bytes),
        "pdf" => parse_pdf(bytes),
        "md" => Ok(parse_markdown(&decode_text(bytes))),
        _ => Ok(ParsedFile::new(
            split_lines(&decode_text(bytes))
                .map(SourceLine::new)
                .collect(),
        )),
    }
}

/// UTF-8 (with or without BOM), UTF-16 with BOM, otherwise GB18030 (a superset of GBK).
pub(crate) fn decode_text(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(b"\xEF\xBB\xBF") {
        return String::from_utf8_lossy(rest).into_owned();
    }
    if let Some(rest) = bytes.strip_prefix(b"\xFF\xFE") {
        return encoding_rs::UTF_16LE
            .decode_without_bom_handling(rest)
            .0
            .into_owned();
    }
    if let Some(rest) = bytes.strip_prefix(b"\xFE\xFF") {
        return encoding_rs::UTF_16BE
            .decode_without_bom_handling(rest)
            .0
            .into_owned();
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => encoding_rs::GB18030
            .decode_without_bom_handling(bytes)
            .0
            .into_owned(),
    }
}

fn split_lines(text: &str) -> impl Iterator<Item = &str> {
    text.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l))
}

fn parse_docx(bytes: &[u8]) -> Result<ParsedFile> {
    let doc = docx_engine::Document::from_bytes(bytes.to_vec())
        .map_err(|e| Error::Parse(e.to_string()))?;
    let mut title_hint = None;
    let lines = doc
        .paragraphs
        .iter()
        .enumerate()
        .map(|(i, p)| {
            // Soft line breaks become spaces so line i stays paragraph i.
            let text = p.accepted_text().replace(['\n', '\r'], " ");
            let is_title = p.style_id.as_deref().is_some_and(|id| {
                id.eq_ignore_ascii_case("title")
                    || doc
                        .style_name(id)
                        .is_some_and(|n| n.eq_ignore_ascii_case("title") || n == "标题")
            });
            if is_title && title_hint.is_none() && !text.trim().is_empty() {
                title_hint = Some(text.trim().to_string());
            }
            let display = doc
                .list_label(i)
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(|label| {
                    let sep = if label.ends_with(char::is_alphanumeric) {
                        " "
                    } else {
                        ""
                    };
                    format!("{label}{sep}{}", text.trim_start())
                });
            SourceLine {
                style_level: if is_title { None } else { doc.heading_level(i) },
                text,
                display,
            }
        })
        .collect();
    Ok(ParsedFile {
        title_hint,
        ..ParsedFile::new(lines)
    })
}

static MD_HEADING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^ {0,3}(#{1,6})(?:[ \t]+(.*?))?(?:[ \t]+#+)?[ \t]*$").unwrap());

fn parse_markdown(text: &str) -> ParsedFile {
    let mut in_fence = false;
    let mut title_hint = None;
    let mut seen_content = false;
    let lines = split_lines(text)
        .map(|raw| {
            let trimmed = raw.trim_start();
            if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
                in_fence = !in_fence;
            } else if !in_fence && let Some(c) = MD_HEADING.captures(raw) {
                let level = c[1].len() as u8 - 1;
                let heading = c.get(2).map_or("", |m| m.as_str()).trim().to_string();
                if level == 0 && !seen_content && title_hint.is_none() && !heading.is_empty() {
                    title_hint = Some(heading.clone());
                }
                seen_content = true;
                return SourceLine {
                    text: raw.to_string(),
                    style_level: Some(level),
                    display: Some(heading),
                };
            }
            seen_content |= !raw.trim().is_empty();
            SourceLine::new(raw)
        })
        .collect();
    ParsedFile {
        title_hint,
        ..ParsedFile::new(lines)
    }
}

/// "- 3 -", "第 3 页", "第3页 共10页", "3/10".
static PAGE_NUMBER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"^([-—–－]+\s*[0-9]{1,4}\s*[-—–－]+",
        r"|第\s*[0-9]{1,4}\s*页(\s*[,，]?\s*共\s*[0-9]{1,4}\s*页)?",
        r"|[0-9]{1,4}\s*/\s*[0-9]{1,4})$"
    ))
    .unwrap()
});

fn parse_pdf(bytes: &[u8]) -> Result<ParsedFile> {
    let (pages, failed) = catch_unwind(AssertUnwindSafe(|| extract_pdf_pages(bytes)))
        .map_err(|_| Error::Parse("PDF 文字提取失败，文件可能已损坏".into()))??;
    let chars: usize = pages
        .iter()
        .map(|p| p.chars().filter(|c| !c.is_whitespace()).count())
        .sum();
    let mut parsed = ParsedFile::new(unwrap_pdf_lines(&pages));
    if !failed.is_empty() {
        let mut list: Vec<String> = failed.iter().take(10).map(u32::to_string).collect();
        if failed.len() > 10 {
            list.push(format!("等 {} 页", failed.len()));
        }
        parsed.warnings.push(format!(
            "第 {} 页文字提取失败（可能使用了不支持的字体编码）",
            list.join("、")
        ));
    }
    if chars < MIN_PDF_CHARS_PER_PAGE * pages.len().max(1) {
        parsed.no_text = true;
        if failed.is_empty() {
            parsed.warnings.push(SCANNED_PDF_WARNING.to_string());
        }
    }
    Ok(parsed)
}

/// Text of every page, and the numbers of pages whose text could not be extracted.
fn extract_pdf_pages(bytes: &[u8]) -> Result<(Vec<String>, Vec<u32>)> {
    let mut doc =
        pdf_extract::Document::load_mem(bytes).map_err(|e| Error::Parse(format!("PDF：{e}")))?;
    if doc.is_encrypted() && doc.decrypt("").is_err() {
        return Err(Error::Parse("PDF 已加密，需要密码才能读取".into()));
    }
    let mut pages = Vec::new();
    let mut failed = Vec::new();
    for page in doc.get_pages().into_keys() {
        let mut text = String::new();
        // pdf-extract panics on some fonts and encodings it does not support.
        let result = catch_unwind(AssertUnwindSafe(|| {
            let mut out = pdf_extract::PlainTextOutput::new(&mut text);
            pdf_extract::output_doc_page(&doc, &mut out, page)
        }));
        if !matches!(result, Ok(Ok(()))) {
            failed.push(page);
        }
        pages.push(text);
    }
    Ok((pages, failed))
}

/// Joins lines that a PDF wrapped at the page width back into paragraphs,
/// drops page numbers and closes up letter-spaced Chinese text.
///
/// Blank lines carry no meaning (text extraction emits one wherever the line
/// spacing is wide), so a line continues into the next when it ends
/// mid-clause (with a comma) or runs to the full text width without closing
/// punctuation, and neither line is a standalone heading.
fn unwrap_pdf_lines(pages: &[String]) -> Vec<SourceLine> {
    let mut lines: Vec<String> = Vec::new();
    for page in pages {
        let page_lines: Vec<&str> = page
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        let last = page_lines.len().saturating_sub(1);
        for (i, line) in page_lines.into_iter().enumerate() {
            // A bare number is a page number only at the top or bottom of a page.
            let bare_number = line.len() <= 4 && line.chars().all(|c| c.is_ascii_digit());
            if PAGE_NUMBER.is_match(line) || (bare_number && (i == 0 || i == last)) {
                continue;
            }
            lines.push(close_up_cjk(line));
        }
    }
    let mut lengths: Vec<usize> = lines.iter().map(|l| l.chars().count()).collect();
    lengths.sort_unstable();
    let full_width = lengths.get(lengths.len() * 9 / 10).copied().unwrap_or(0);

    let mut out: Vec<String> = Vec::new();
    let mut open = false;
    for line in &lines {
        let heading = detect_heading(line);
        match out.last_mut() {
            Some(prev) if open && heading.is_none() => {
                let joins_words = prev.ends_with(|c: char| c.is_ascii_alphanumeric())
                    && line.starts_with(|c: char| c.is_ascii_alphanumeric());
                if joins_words {
                    prev.push(' ');
                }
                prev.push_str(line);
            }
            _ => out.push(line.clone()),
        }
        let ends_clause = line.ends_with(['，', '、', ',']);
        let ends_sentence = line.ends_with([
            '。', '！', '？', '；', '：', '!', '?', ';', ':', '”', '」', '）',
        ]);
        // Short lines (table cells, lists) never count as wrapped.
        let full = full_width >= MIN_WRAP_WIDTH && line.chars().count() * 10 >= full_width * 8;
        // Articles and run-in headings continue on the next line; titles do not.
        open = heading.is_none_or(|h| h.has_body) && (ends_clause || (full && !ends_sentence));
    }
    out.into_iter().map(SourceLine::new).collect()
}

/// Removes the spaces text extraction puts between Chinese characters of
/// justified or letter-spaced text ("数 据 安 全"), when there are many;
/// a single gap (after "第一条", between two issuers) is kept.
fn close_up_cjk(line: &str) -> String {
    let chars: Vec<char> = line.chars().collect();
    let cjk = |c: char| is_han(c) || matches!(c, '\u{3000}'..='\u{303F}' | '\u{FF00}'..='\u{FFEF}');
    let gap = |i: usize| {
        chars[i] == ' ' && i > 0 && i + 1 < chars.len() && cjk(chars[i - 1]) && cjk(chars[i + 1])
    };
    let gaps = (0..chars.len()).filter(|&i| gap(i)).count();
    let han = chars.iter().filter(|&&c| is_han(c)).count();
    if gaps < 3 || gaps * 4 < han {
        return line.to_string();
    }
    (0..chars.len())
        .filter(|&i| !gap(i))
        .map(|i| chars[i])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closes_up_letter_spaced_chinese() {
        assert_eq!(close_up_cjk("数 据 安 全 管 理 办 法"), "数据安全管理办法");
        assert_eq!(
            close_up_cjk("第一条 为了规范数据处理活动"),
            "第一条 为了规范数据处理活动"
        );
        assert_eq!(close_up_cjk("财政部 税务总局"), "财政部 税务总局");
        assert_eq!(
            close_up_cjk("GB/T 22239 等 级 保 护"),
            "GB/T 22239 等级保护"
        );
    }

    #[test]
    fn drops_page_numbers_but_keeps_table_numbers() {
        let pages = [
            "12\n序号\n1\n数据名称\n".to_string(),
            "- 13 -\n正文。\n第 14 页 共 20 页".to_string(),
        ];
        let lines: Vec<String> = unwrap_pdf_lines(&pages)
            .into_iter()
            .map(|l| l.text)
            .collect();
        assert_eq!(lines, ["序号", "1", "数据名称", "正文。"]);
    }

    #[test]
    fn detects_formats() {
        assert_eq!(format_of(Path::new("a.DOCX")).unwrap(), "docx");
        assert_eq!(format_of(Path::new("a.markdown")).unwrap(), "md");
        let err = format_of(Path::new("a.doc")).unwrap_err().to_string();
        assert!(err.starts_with("不支持的格式：.doc"), "{err}");
    }
}
