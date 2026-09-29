//! Reading supported file formats into lines.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
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
        "png" | "jpg" | "jpeg" => Err(Error::NeedsEnhanced),
        "" => Err(Error::UnsupportedFormat("无扩展名".into())),
        other => Err(Error::UnsupportedFormat(format!(".{other}"))),
    }
}

/// Extensions the built-in parsers read, lowercase.
pub const SUPPORTED_EXTENSIONS: &[&str] = &["docx", "pdf", "txt", "text", "md", "markdown"];
/// Image extensions an online parser reads, lowercase.
pub const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg"];
/// Files larger than this are skipped when a folder is imported.
pub const MAX_FILE_BYTES: u64 = 200 * 1024 * 1024;

/// The files to import for the paths the user picked or dropped.
///
/// A file is kept as given, whatever its type, so an unsupported one is
/// reported instead of silently dropped. A folder is walked recursively
/// without following symbolic links, keeping files with a supported
/// extension (plus PNG/JPEG when `images` is set) and skipping hidden
/// files and folders (`.name`), Office lock files (`~$name`) and files over
/// [`MAX_FILE_BYTES`]. Paths keep the order given, a folder's files come
/// sorted by path, and a file reached twice is listed once.
pub fn expand_import_paths(paths: &[PathBuf], images: bool) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for path in paths {
        if path.is_dir() {
            let mut found = Vec::new();
            walk_folder(path, images, &mut found);
            found.sort();
            out.extend(found.into_iter().filter(|p| seen.insert(p.clone())));
        } else if seen.insert(path.clone()) {
            out.push(path.clone());
        }
    }
    out
}

fn walk_folder(dir: &Path, images: bool, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || name.starts_with("~$") {
            continue;
        }
        // `file_type` does not follow symbolic links.
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let path = entry.path();
        if kind.is_dir() {
            walk_folder(&path, images, out);
        } else if kind.is_file() {
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .map(str::to_ascii_lowercase)
                .unwrap_or_default();
            let wanted = SUPPORTED_EXTENSIONS.contains(&ext.as_str())
                || (images && IMAGE_EXTENSIONS.contains(&ext.as_str()));
            let small = entry.metadata().is_ok_and(|m| m.len() <= MAX_FILE_BYTES);
            if wanted && small {
                out.push(path);
            }
        }
    }
}

/// Format name for a file whose text comes from an online parser: the
/// built-in formats plus `image` for PNG and JPEG.
pub(crate) fn format_of_parsed(path: &Path) -> Result<&'static str> {
    match format_of(path) {
        Err(Error::NeedsEnhanced) => Ok("image"),
        other => other,
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
    let mut lines = Vec::new();
    for block in &doc.blocks {
        match block {
            docx_engine::Block::Paragraph(i) => {
                let i = *i;
                let p = &doc.paragraphs[i];
                // Soft line breaks become spaces so a paragraph stays one line.
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
                lines.push(SourceLine {
                    style_level: if is_title { None } else { doc.heading_level(i) },
                    text,
                    display,
                });
            }
            docx_engine::Block::Table(t) => {
                lines.extend(table_lines(&doc, t).into_iter().map(SourceLine::new));
            }
        }
    }
    Ok(ParsedFile {
        title_hint,
        ..ParsedFile::new(lines)
    })
}

/// A table as text a search can use: one line per row, each value labelled
/// with its column header ("项目：硬件购置；金额（万元）：1260"), so a chunk
/// holding any rows still says what the numbers mean. Merged cells repeat
/// their text in every row and column they cover.
fn table_lines(doc: &docx_engine::Document, table: &docx_engine::Table) -> Vec<String> {
    let cell_text = |c: &docx_engine::TableCell| {
        c.paragraphs
            .iter()
            .map(|&i| doc.paragraphs[i].accepted_text().replace(['\n', '\r'], " "))
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    };
    let mut grid: Vec<Vec<String>> = Vec::new();
    for row in &table.rows {
        let mut out = Vec::new();
        for cell in row {
            let col = out.len();
            let text = if cell.v_merge == Some(false) {
                grid.last()
                    .and_then(|above: &Vec<String>| above.get(col).cloned())
                    .unwrap_or_default()
            } else {
                cell_text(cell)
            };
            for _ in 0..cell.grid_span.max(1) {
                out.push(text.clone());
            }
        }
        grid.push(out);
    }
    grid_lines(grid)
}

/// Lines for a table given as a grid of cell texts, merged cells already
/// repeated: the first row is the header, every other row becomes
/// "列名：值；列名：值". Tables with fewer than two rows or columns have no
/// header to label values with and give one line per row.
fn grid_lines(mut grid: Vec<Vec<String>>) -> Vec<String> {
    grid.retain(|r| r.iter().any(|c| !c.is_empty()));
    let columns = grid.iter().map(Vec::len).max().unwrap_or(0);
    // Plain rows when there is no header to label values with.
    if grid.len() < 2 || columns < 2 {
        return grid.iter().map(|r| join_distinct(r, " | ")).collect();
    }
    let header = &grid[0];
    let mut out = vec![format!("表格列：{}", join_distinct(header, " | "))];
    for row in &grid[1..] {
        let mut parts: Vec<String> = Vec::new();
        for (col, value) in row.iter().enumerate() {
            let name = header.get(col).map_or("", String::as_str);
            let part = match (name.is_empty() || name == value, value.is_empty()) {
                (_, true) => continue,
                (true, false) => value.clone(),
                (false, false) => format!("{name}：{value}"),
            };
            if parts.last() != Some(&part) {
                parts.push(part);
            }
        }
        if !parts.is_empty() {
            out.push(parts.join("；"));
        }
    }
    out
}

/// Joins non-empty cells, dropping repeats left by merged cells.
fn join_distinct(cells: &[String], sep: &str) -> String {
    let mut kept: Vec<&str> = Vec::new();
    for c in cells {
        if !c.is_empty() && kept.last() != Some(&c.as_str()) {
            kept.push(c);
        }
    }
    kept.join(sep)
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
                    // Chunks and citations show the heading without `#`.
                    text: if heading.is_empty() {
                        raw.to_string()
                    } else {
                        heading.clone()
                    },
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

/// Markdown from an online parser (MinerU, PaddleOCR) as a parsed file.
pub(crate) fn parse_external(md: &str) -> ParsedFile {
    let mut parsed = parse_markdown(&external_to_markdown(md));
    for line in &mut parsed.lines {
        // Online parsers mark every heading with `#`, often all at one
        // level; numbered headings (第一章, 一、) keep their natural ranks.
        if line.style_level.is_some()
            && detect_heading(line.display.as_deref().unwrap_or(&line.text))
                .is_some_and(|h| h.kind.rank().is_some())
        {
            line.style_level = None;
        }
    }
    parsed
}

/// Lines from the Markdown an online parser (MinerU, PaddleOCR) returns,
/// ready for chunking like the built-in formats: `#` headings become
/// heading lines, HTML and pipe tables become "列名：值" rows as for Word
/// tables, and images, HTML comments and formatting tags are dropped.
pub fn parse_external_markdown(md: &str) -> Vec<SourceLine> {
    parse_external(md).lines
}

static HTML_COMMENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<!--.*?-->").unwrap());
static HTML_TABLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<table\b[^>]*>.*?</table\s*>").unwrap());
static HTML_ROW: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<tr\b[^>]*>(.*?)</tr\s*>").unwrap());
static HTML_CELL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<t[dh]\b([^>]*)>(.*?)</t[dh]\s*>").unwrap());
static ROWSPAN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)\browspan\s*=\s*["']?\s*(\d+)"#).unwrap());
static COLSPAN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)\bcolspan\s*=\s*["']?\s*(\d+)"#).unwrap());
static HTML_BREAK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)<br\s*/?>|</?(?:p|div)\b[^>]*>").unwrap());
static HTML_TAG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(concat!(
        r"(?i)</?(?:span|sup|sub|b|i|u|s|em|strong|font|center|small|big|mark|a|img",
        r"|html|body|thead|tbody|tfoot|caption|colgroup|col)\b[^>]*>"
    ))
    .unwrap()
});
static MD_IMAGE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"!\[[^\]\n]*\]\([^)\n]*\)").unwrap());
static PIPE_SEPARATOR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\|?\s*:?-{2,}:?\s*(\|\s*:?-{2,}:?\s*)*\|?$").unwrap());
static ENTITY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"&(#[0-9]{1,7}|#[xX][0-9a-fA-F]{1,6}|[a-zA-Z]{2,8});").unwrap());

/// Rewrites online-parser Markdown into plain Markdown whose tables are
/// already row lines.
fn external_to_markdown(md: &str) -> String {
    let md = md.replace("\r\n", "\n");
    let md = HTML_COMMENT.replace_all(&md, "");
    // Tables stand on their own lines.
    let md = HTML_TABLE.replace_all(&md, |c: &regex::Captures| {
        format!("\n{}\n", html_table_lines(&c[0]).join("\n"))
    });
    let lines: Vec<&str> = md.split('\n').collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut in_fence = false;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            out.push(line.to_string());
            i += 1;
            continue;
        }
        if !in_fence
            && trimmed.contains('|')
            && lines
                .get(i + 1)
                .is_some_and(|next| PIPE_SEPARATOR.is_match(next.trim()))
        {
            let mut grid = vec![pipe_cells(trimmed)];
            i += 2;
            while let Some(row) = lines.get(i).map(|l| l.trim())
                && !row.is_empty()
                && row.contains('|')
            {
                grid.push(pipe_cells(row));
                i += 1;
            }
            let width = grid.iter().map(Vec::len).max().unwrap_or(0);
            for row in &mut grid {
                row.resize(width, String::new());
            }
            out.extend(grid_lines(grid));
            continue;
        }
        out.push(if in_fence {
            line.to_string()
        } else {
            clean_inline(line)
        });
        i += 1;
    }
    out.join("\n")
}

/// A line without images and formatting tags, entities decoded.
fn clean_inline(line: &str) -> String {
    let line = MD_IMAGE.replace_all(line, "");
    let line = HTML_BREAK.replace_all(&line, " ");
    let line = HTML_TAG.replace_all(&line, "");
    let line = unescape_entities(&line);
    // A line that held only an image is empty now, not whitespace.
    if line.trim().is_empty() {
        String::new()
    } else {
        line.trim_end().to_string()
    }
}

/// Cells of a pipe-table row; `\|` is a literal bar.
fn pipe_cells(row: &str) -> Vec<String> {
    let row = row.strip_prefix('|').unwrap_or(row);
    let row = row.strip_suffix('|').unwrap_or(row);
    let mut cells = Vec::new();
    let mut cell = String::new();
    let mut chars = row.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if chars.peek() == Some(&'|') => {
                cell.push('|');
                chars.next();
            }
            '|' => cells.push(std::mem::take(&mut cell)),
            c => cell.push(c),
        }
    }
    cells.push(cell);
    cells.iter().map(|c| cell_text(c)).collect()
}

/// Text of a table cell on one line.
fn cell_text(html: &str) -> String {
    let text = clean_inline(&HTML_BREAK.replace_all(html, " "));
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Rows of an HTML table as lines, with `rowspan` and `colspan` cells
/// repeated in every row and column they cover.
fn html_table_lines(table: &str) -> Vec<String> {
    let mut grid: Vec<Vec<String>> = Vec::new();
    // Cells from earlier rows reaching down: (column, text, rows left).
    let mut pending: Vec<(usize, String, usize)> = Vec::new();
    for row in HTML_ROW.captures_iter(table) {
        let mut out: Vec<Option<String>> = Vec::new();
        for (col, text, left) in &mut pending {
            if out.len() <= *col {
                out.resize(*col + 1, None);
            }
            out[*col] = Some(text.clone());
            *left -= 1;
        }
        pending.retain(|(_, _, left)| *left > 0);
        let mut col = 0;
        for cell in HTML_CELL.captures_iter(&row[1]) {
            let attrs = &cell[1];
            let span = |re: &Regex| {
                re.captures(attrs)
                    .and_then(|c| c[1].parse::<usize>().ok())
                    .unwrap_or(1)
                    .clamp(1, 1000)
            };
            let (rowspan, colspan) = (span(&ROWSPAN), span(&COLSPAN));
            let text = cell_text(&cell[2]);
            while out.get(col).is_some_and(Option::is_some) {
                col += 1;
            }
            for c in col..col + colspan {
                if out.len() <= c {
                    out.resize(c + 1, None);
                }
                out[c] = Some(text.clone());
                if rowspan > 1 {
                    pending.push((c, text.clone(), rowspan - 1));
                }
            }
            col += colspan;
        }
        grid.push(out.into_iter().map(Option::unwrap_or_default).collect());
    }
    let width = grid.iter().map(Vec::len).max().unwrap_or(0);
    for row in &mut grid {
        row.resize(width, String::new());
    }
    grid_lines(grid)
}

/// Decodes the entities online parsers emit: the XML five, `&nbsp;`, common
/// typographic names and numeric references. Unknown names stay as written.
fn unescape_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    ENTITY
        .replace_all(s, |c: &regex::Captures| {
            let name = &c[1];
            let decoded = if let Some(num) = name.strip_prefix('#') {
                let n = match num.strip_prefix(['x', 'X']) {
                    Some(hex) => u32::from_str_radix(hex, 16).ok(),
                    None => num.parse().ok(),
                };
                n.and_then(char::from_u32)
            } else {
                match name {
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "quot" => Some('"'),
                    "apos" => Some('\''),
                    "nbsp" | "ensp" | "emsp" | "thinsp" => Some(' '),
                    "ldquo" => Some('“'),
                    "rdquo" => Some('”'),
                    "lsquo" => Some('‘'),
                    "rsquo" => Some('’'),
                    "middot" => Some('·'),
                    "times" => Some('×'),
                    "divide" => Some('÷'),
                    "plusmn" => Some('±'),
                    "le" => Some('≤'),
                    "ge" => Some('≥'),
                    "deg" => Some('°'),
                    "mdash" => Some('—'),
                    "ndash" => Some('–'),
                    "hellip" => Some('…'),
                    _ => None,
                }
            };
            decoded.map_or_else(|| c[0].to_string(), String::from)
        })
        .into_owned()
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
