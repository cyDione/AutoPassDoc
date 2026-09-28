//! Generates synthetic review documents for tests and benchmarks: a long
//! Chinese report with numbered headings, tables, an image, tracked changes
//! and many comments from several reviewers, including replies and resolved ones.

use std::io::{Cursor, Write};

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

#[derive(Debug, Clone)]
pub struct Spec {
    /// Minimum number of non-whitespace characters in the body.
    pub target_chars: usize,
    /// Number of top-level comments (replies come on top).
    pub comments: usize,
    pub seed: u64,
}

impl Default for Spec {
    fn default() -> Self {
        Self {
            target_chars: 220_000,
            comments: 500,
            seed: 7,
        }
    }
}

pub const AUTHORS: &[&str] = &[
    "张主任",
    "李教授",
    "王处长",
    "赵工",
    "Administrator",
    "user",
    "陈专家",
    "刘研究员",
];

const SENTENCES: &[&str] = &[
    "为深入贯彻落实国家关于数字经济高质量发展的决策部署，本项目围绕数据要素市场化配置改革开展系统研究。",
    "2023年全市规模以上工业增加值同比增长6.8%，高于全国平均水平1.2个百分点。",
    "项目建设内容包括数据中台、业务应用系统、安全保障体系和运营管理机制四个部分。",
    "根据《关于构建数据基础制度更好发挥数据要素作用的意见》，应建立数据产权、流通交易、收益分配和安全治理制度。",
    "通过调研发现，现有信息系统存在数据标准不统一、共享渠道不畅通、应用场景不丰富等突出问题。",
    "预计项目建成后，每年可节约行政成本约1200万元，群众办事平均时长缩短40%以上。",
    "本方案坚持统筹规划、分步实施、急用先行、安全可控的原则，确保项目有序推进。",
    "技术路线采用云原生架构，基于微服务和容器化部署，支持弹性扩展和灰度发布。",
    "数据安全方面，严格落实网络安全等级保护2.0要求，对敏感数据实施分类分级管理。",
    "项目总投资估算为3850万元，其中硬件设备购置费1260万元，软件开发费1720万元，其他费用870万元。",
    "资金来源为市级财政资金，按照项目实施进度分年度拨付。",
    "组织保障方面，成立由分管副市长任组长的项目建设领导小组，统筹协调重大事项。",
    "风险分析表明，项目主要面临技术选型、数据质量、进度控制和运维保障四类风险。",
    "针对上述风险，本方案提出了相应的防范措施和应急预案，确保风险总体可控。",
    "效益分析显示，项目具有显著的社会效益和一定的经济效益，建设必要性充分。",
    "需求分析阶段共走访了市直部门23个、区县政府9个，收集各类业务需求186项。",
    "系统设计遵循高内聚、低耦合的原则，各子系统之间通过标准接口进行数据交换。",
    "运维保障采用“7×24小时”值守机制，重大故障响应时间不超过30分钟。",
    "绩效目标包括产出指标、效益指标和满意度指标三类，共设置二级指标12项。",
    "下一步，将按照“边建设、边应用、边完善”的思路，持续推动系统迭代升级。",
    "与周边城市相比，我市在数据开放数量和应用场景丰富度方面仍有一定差距。",
    "参照国家标准GB/T 36073-2018《数据管理能力成熟度评估模型》，开展数据管理能力评估。",
];

const REMARKS: &[&str] = &[
    "此处数据口径需与统计年鉴保持一致，请核实并注明来源。",
    "建议补充政策依据，引用最新文件文号。",
    "表述过于口语化，请按公文规范修改。",
    "投资估算与附表不一致，请统一。",
    "该段逻辑不够清晰，建议调整结构，先讲问题再讲措施。",
    "“显著”一词缺乏量化支撑，建议给出具体指标。",
    "请补充与国内先进地区的对标分析。",
    "风险应对措施过于笼统，请细化到责任单位和时间节点。",
    "此处引用的标准已废止，请更新为现行版本。",
    "建议删除重复内容，与第二章表述合并。",
    "格式问题：数字与单位之间不应空格，请统一。",
    "请说明测算方法和主要参数取值依据。",
];

const REPLIES: &[&str] = &[
    "已修改。",
    "已补充数据来源。",
    "已按意见调整结构。",
    "已核实，数据无误。",
];

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }
    fn pick<'a>(&mut self, items: &[&'a str]) -> &'a str {
        items[self.below(items.len())]
    }
}

enum Piece {
    Text(String, bool),
    Ins(String),
    Del(String),
    Start(usize),
    End(usize),
    Ref(usize),
    Image,
}

struct CommentSpec {
    author: &'static str,
    text: String,
    parent: Option<usize>,
    done: bool,
}

pub fn generate(spec: &Spec) -> Vec<u8> {
    let mut rng = Rng(spec.seed.max(1).wrapping_mul(0x9E37_79B9_7F4A_7C15));
    let mut comments: Vec<CommentSpec> = Vec::new();
    let mut body = String::new();
    let mut chars = 0usize;
    let mut paragraphs = 0usize;
    // Estimate how many body paragraphs we will write so comments spread evenly.
    let expected_paragraphs = (spec.target_chars / 190).max(1);
    let comment_every = (expected_paragraphs as f64 / spec.comments.max(1) as f64).max(1.0);
    let mut next_comment_at = 0.0f64;
    let mut open_span: Option<usize> = None;

    let mut chapter = 0;
    while chars < spec.target_chars {
        chapter += 1;
        body.push_str(&heading(1, &format!("第{chapter}部分 项目建设方案要点")));
        for section in 1..=4 {
            body.push_str(&heading(
                2,
                &format!("建设内容与实施路径（{chapter}.{section}）"),
            ));
            for sub in 1..=3 {
                body.push_str(&heading(3, &format!("重点任务{sub}")));
                for _ in 0..(3 + rng.below(4)) {
                    let mut text = String::new();
                    for _ in 0..(3 + rng.below(4)) {
                        text.push_str(rng.pick(SENTENCES));
                    }
                    chars += text.chars().filter(|c| !c.is_whitespace()).count();
                    let mut pieces = Vec::new();
                    if let Some(id) = open_span.take() {
                        let cut = char_cut(&text, 0.3);
                        pieces.push(Piece::Text(text[..cut].to_string(), false));
                        pieces.push(Piece::End(id));
                        pieces.push(Piece::Ref(id));
                        pieces.push(Piece::Text(text[cut..].to_string(), false));
                    } else if comments.iter().filter(|c| c.parent.is_none()).count() < spec.comments
                        && paragraphs as f64 >= next_comment_at
                    {
                        next_comment_at += comment_every;
                        let id = add_comment(&mut rng, &mut comments);
                        let a = char_cut(&text, 0.2 + rng.below(40) as f64 / 100.0);
                        let b = char_cut(&text, 0.65 + rng.below(30) as f64 / 100.0);
                        pieces.push(Piece::Text(text[..a].to_string(), false));
                        pieces.push(Piece::Start(id));
                        if rng.chance(5) {
                            // Range continues into the next paragraph.
                            pieces.push(Piece::Text(text[a..].to_string(), false));
                            open_span = Some(id);
                        } else {
                            pieces.push(Piece::Text(text[a..b].to_string(), false));
                            pieces.push(Piece::End(id));
                            pieces.push(Piece::Ref(id));
                            pieces.push(Piece::Text(text[b..].to_string(), false));
                        }
                    } else if rng.chance(3) {
                        let a = char_cut(&text, 0.4);
                        pieces.push(Piece::Text(text[..a].to_string(), false));
                        pieces.push(Piece::Del("原有表述".to_string()));
                        pieces.push(Piece::Ins("修订后的表述".to_string()));
                        pieces.push(Piece::Text(text[a..].to_string(), false));
                    } else if rng.chance(10) {
                        let a = char_cut(&text, 0.5);
                        pieces.push(Piece::Text("重点说明：".to_string(), true));
                        pieces.push(Piece::Text(text[..a].to_string(), false));
                        pieces.push(Piece::Text(text[a..].to_string(), false));
                    } else {
                        pieces.push(Piece::Text(text, false));
                    }
                    body.push_str(&paragraph(&pieces, None, &comments));
                    paragraphs += 1;
                }
                if paragraphs == 5 {
                    body.push_str(&paragraph(&[Piece::Image], Some("center"), &comments));
                }
            }
            if section % 2 == 0 {
                let commented =
                    comments.iter().filter(|c| c.parent.is_none()).count() < spec.comments;
                let id = commented.then(|| add_comment(&mut rng, &mut comments));
                body.push_str(&table(&mut rng, id, &comments, &mut chars));
            }
        }
    }
    if let Some(id) = open_span.take() {
        body.push_str(&paragraph(
            &[
                Piece::Text("（全文完）".into(), false),
                Piece::End(id),
                Piece::Ref(id),
            ],
            None,
            &comments,
        ));
    }

    let document = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture"><w:body>{body}<w:sectPr><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1440" w:right="1800" w:bottom="1440" w:left="1800" w:header="851" w:footer="992" w:gutter="0"/></w:sectPr></w:body></w:document>"#
    );

    let mut comments_xml = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:comments xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:w14="http://schemas.microsoft.com/office/word/2010/wordml">"#,
    );
    let mut extended = String::from(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w15:commentsEx xmlns:w15="http://schemas.microsoft.com/office/word/2012/wordml">"#,
    );
    for (i, c) in comments.iter().enumerate() {
        let day = 1 + i % 28;
        comments_xml.push_str(&format!(
            r#"<w:comment w:id="{i}" w:author="{}" w:date="2026-09-{day:02}T10:00:00Z" w:initials="{}"><w:p w14:paraId="{}"><w:r><w:annotationRef/></w:r><w:r><w:t xml:space="preserve">{}</w:t></w:r></w:p></w:comment>"#,
            escape(c.author),
            escape(&c.author.chars().take(1).collect::<String>()),
            para_id(i),
            escape(&c.text)
        ));
        let parent = c
            .parent
            .map(|p| format!(r#" w15:paraIdParent="{}""#, para_id(p)))
            .unwrap_or_default();
        extended.push_str(&format!(
            r#"<w15:commentEx w15:paraId="{}"{parent} w15:done="{}"/>"#,
            para_id(i),
            u8::from(c.done)
        ));
    }
    comments_xml.push_str("</w:comments>");
    extended.push_str("</w15:commentsEx>");

    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .last_modified_time(zip::DateTime::default());
    let mut add = |name: &str, data: &[u8]| {
        zip.start_file(name, options).unwrap();
        zip.write_all(data).unwrap();
    };
    add("[Content_Types].xml", CONTENT_TYPES.as_bytes());
    add("_rels/.rels", ROOT_RELS.as_bytes());
    add("word/document.xml", document.as_bytes());
    add("word/_rels/document.xml.rels", DOCUMENT_RELS.as_bytes());
    add("word/styles.xml", STYLES.as_bytes());
    add("word/numbering.xml", NUMBERING.as_bytes());
    add("word/comments.xml", comments_xml.as_bytes());
    add("word/commentsExtended.xml", extended.as_bytes());
    add("word/media/image1.png", &sample_png(320, 180));
    zip.finish().unwrap().into_inner()
}

fn add_comment(rng: &mut Rng, comments: &mut Vec<CommentSpec>) -> usize {
    let id = comments.len();
    comments.push(CommentSpec {
        author: AUTHORS[rng.below(AUTHORS.len())],
        text: rng.pick(REMARKS).to_string(),
        parent: None,
        done: rng.chance(10),
    });
    if rng.chance(20) {
        comments.push(CommentSpec {
            author: "报告撰写人",
            text: rng.pick(REPLIES).to_string(),
            parent: Some(id),
            done: false,
        });
    }
    id
}

/// Ids of a comment and its replies, which share the same range in Word.
fn thread_ids(id: usize, comments: &[CommentSpec]) -> Vec<usize> {
    std::iter::once(id)
        .chain((id + 1..comments.len()).take_while(|&i| comments[i].parent == Some(id)))
        .collect()
}

fn char_cut(text: &str, fraction: f64) -> usize {
    let n = text.chars().count();
    let k = ((n as f64) * fraction.clamp(0.0, 1.0)) as usize;
    text.char_indices().nth(k).map_or(text.len(), |(i, _)| i)
}

fn heading(level: u8, text: &str) -> String {
    format!(
        r#"<w:p><w:pPr><w:pStyle w:val="Heading{level}"/></w:pPr><w:r><w:t xml:space="preserve">{}</w:t></w:r></w:p>"#,
        escape(text)
    )
}

fn run(text: &str, bold: bool) -> String {
    let rpr = if bold { "<w:rPr><w:b/></w:rPr>" } else { "" };
    format!(
        r#"<w:r>{rpr}<w:t xml:space="preserve">{}</w:t></w:r>"#,
        escape(text)
    )
}

fn paragraph(pieces: &[Piece], align: Option<&str>, comments: &[CommentSpec]) -> String {
    let mut out = String::from("<w:p>");
    if let Some(a) = align {
        out.push_str(&format!(r#"<w:pPr><w:jc w:val="{a}"/></w:pPr>"#));
    }
    for piece in pieces {
        match piece {
            Piece::Text(t, bold) if !t.is_empty() => out.push_str(&run(t, *bold)),
            Piece::Text(..) => {}
            Piece::Ins(t) => out.push_str(&format!(
                r#"<w:ins w:id="9{}" w:author="李教授" w:date="2026-09-01T00:00:00Z">{}</w:ins>"#,
                out.len(),
                run(t, false)
            )),
            Piece::Del(t) => out.push_str(&format!(
                r#"<w:del w:id="8{}" w:author="李教授" w:date="2026-09-01T00:00:00Z"><w:r><w:delText xml:space="preserve">{}</w:delText></w:r></w:del>"#,
                out.len(),
                escape(t)
            )),
            Piece::Start(id) => {
                for i in thread_ids(*id, comments) {
                    out.push_str(&format!(r#"<w:commentRangeStart w:id="{i}"/>"#));
                }
            }
            Piece::End(id) => {
                for i in thread_ids(*id, comments) {
                    out.push_str(&format!(r#"<w:commentRangeEnd w:id="{i}"/>"#));
                }
            }
            Piece::Ref(id) => {
                for i in thread_ids(*id, comments) {
                    out.push_str(&format!(
                        r#"<w:r><w:rPr><w:rStyle w:val="CommentReference"/></w:rPr><w:commentReference w:id="{i}"/></w:r>"#
                    ));
                }
            }
            Piece::Image => out.push_str(IMAGE_RUN),
        }
    }
    out.push_str("</w:p>");
    out
}

fn table(
    rng: &mut Rng,
    comment: Option<usize>,
    comments: &[CommentSpec],
    chars: &mut usize,
) -> String {
    let header = ["序号", "建设内容", "投资（万元）", "完成时限"];
    let mut out = String::from(
        r#"<w:tbl><w:tblPr><w:tblStyle w:val="TableGrid"/><w:tblW w:w="0" w:type="auto"/></w:tblPr><w:tblGrid><w:gridCol w:w="1000"/><w:gridCol w:w="4000"/><w:gridCol w:w="1800"/><w:gridCol w:w="1800"/></w:tblGrid>"#,
    );
    let cell = |text: &str, bold: bool, extra: &str| {
        format!(
            r#"<w:tc><w:tcPr><w:tcW w:w="0" w:type="auto"/>{extra}</w:tcPr><w:p>{}</w:p></w:tc>"#,
            run(text, bold)
        )
    };
    out.push_str("<w:tr>");
    for h in header {
        out.push_str(&cell(h, true, ""));
    }
    out.push_str("</w:tr>");
    for row in 1..=4 {
        out.push_str("<w:tr>");
        out.push_str(&cell(&row.to_string(), false, ""));
        let content = rng.pick(SENTENCES);
        *chars += content.chars().count();
        if let (2, Some(id)) = (row, comment) {
            let pieces = [
                Piece::Start(id),
                Piece::Text(content.to_string(), false),
                Piece::End(id),
                Piece::Ref(id),
            ];
            out.push_str(&format!(
                r#"<w:tc><w:tcPr><w:tcW w:w="0" w:type="auto"/></w:tcPr>{}</w:tc>"#,
                paragraph(&pieces, None, comments)
            ));
        } else {
            out.push_str(&cell(content, false, ""));
        }
        if row == 4 {
            out.push_str(&cell("合计另计", false, r#"<w:gridSpan w:val="2"/>"#));
        } else {
            out.push_str(&cell(&format!("{}", 100 + rng.below(900)), false, ""));
            out.push_str(&cell("2027年6月", false, ""));
        }
        out.push_str("</w:tr>");
    }
    out.push_str("</w:tbl>");
    out
}

fn para_id(i: usize) -> String {
    format!("{:08X}", 0x1000_0000 + i)
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// A small gradient PNG, encoded with stored (uncompressed) deflate blocks.
fn sample_png(width: u32, height: u32) -> Vec<u8> {
    let mut raw = Vec::with_capacity(((width * 3 + 1) * height) as usize);
    for y in 0..height {
        raw.push(0);
        for x in 0..width {
            raw.extend([
                (200 - (x * 60 / width)) as u8,
                (190 - (y * 50 / height)) as u8,
                (170 + (x * 40 / width)) as u8,
            ]);
        }
    }
    let mut zlib = vec![0x78, 0x01];
    let mut chunks = raw.chunks(65535).peekable();
    while let Some(chunk) = chunks.next() {
        zlib.push(u8::from(chunks.peek().is_none()));
        let len = chunk.len() as u16;
        zlib.extend(len.to_le_bytes());
        zlib.extend((!len).to_le_bytes());
        zlib.extend(chunk);
    }
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in &raw {
        a = (a + byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    zlib.extend(((b << 16) | a).to_be_bytes());

    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    let mut ihdr = Vec::new();
    ihdr.extend(width.to_be_bytes());
    ihdr.extend(height.to_be_bytes());
    ihdr.extend([8, 2, 0, 0, 0]);
    for (kind, data) in [(b"IHDR", ihdr), (b"IDAT", zlib), (b"IEND", Vec::new())] {
        png.extend((data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend(&data);
        png.extend(&body);
        png.extend(crc32(&body).to_be_bytes());
    }
    png
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in data {
        crc ^= byte as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

const IMAGE_RUN: &str = r#"<w:r><w:drawing><wp:inline distT="0" distB="0" distL="0" distR="0"><wp:extent cx="3048000" cy="1714500"/><wp:docPr id="1" name="图片 1" descr="项目总体架构示意图"/><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic><pic:nvPicPr><pic:cNvPr id="1" name="image1.png"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip r:embed="rIdImage1"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="3048000" cy="1714500"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r>"#;

const CONTENT_TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="png" ContentType="image/png"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/><Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/><Override PartName="/word/numbering.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml"/><Override PartName="/word/comments.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.comments+xml"/><Override PartName="/word/commentsExtended.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.commentsExtended+xml"/></Types>"#;

const ROOT_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;

const DOCUMENT_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering" Target="numbering.xml"/><Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/comments" Target="comments.xml"/><Relationship Id="rId4" Type="http://schemas.microsoft.com/office/2011/relationships/commentsExtended" Target="commentsExtended.xml"/><Relationship Id="rIdImage1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image1.png"/></Relationships>"#;

const STYLES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/><w:pPr><w:spacing w:line="360" w:lineRule="auto"/><w:ind w:firstLineChars="200" w:firstLine="420"/></w:pPr><w:rPr><w:rFonts w:ascii="Times New Roman" w:eastAsia="仿宋"/><w:sz w:val="28"/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr><w:outlineLvl w:val="0"/></w:pPr><w:rPr><w:rFonts w:eastAsia="黑体"/><w:b/><w:sz w:val="32"/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="Heading2"><w:name w:val="heading 2"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:pPr><w:numPr><w:ilvl w:val="1"/><w:numId w:val="1"/></w:numPr><w:outlineLvl w:val="1"/></w:pPr><w:rPr><w:rFonts w:eastAsia="楷体"/><w:b/></w:rPr></w:style><w:style w:type="paragraph" w:styleId="Heading3"><w:name w:val="heading 3"/><w:basedOn w:val="Normal"/><w:next w:val="Normal"/><w:pPr><w:numPr><w:ilvl w:val="2"/><w:numId w:val="1"/></w:numPr><w:outlineLvl w:val="2"/></w:pPr><w:rPr><w:b/></w:rPr></w:style><w:style w:type="character" w:styleId="CommentReference"><w:name w:val="annotation reference"/><w:rPr><w:sz w:val="21"/></w:rPr></w:style><w:style w:type="table" w:styleId="TableGrid"><w:name w:val="Table Grid"/><w:tblPr><w:tblBorders><w:top w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:left w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:bottom w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:right w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:insideH w:val="single" w:sz="4" w:space="0" w:color="auto"/><w:insideV w:val="single" w:sz="4" w:space="0" w:color="auto"/></w:tblBorders></w:tblPr></w:style></w:styles>"#;

const NUMBERING: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:numbering xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:abstractNum w:abstractNumId="0"><w:multiLevelType w:val="multilevel"/><w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="chineseCounting"/><w:lvlText w:val="%1、"/><w:lvlJc w:val="left"/><w:pPr><w:ind w:left="0" w:firstLine="0"/></w:pPr></w:lvl><w:lvl w:ilvl="1"><w:start w:val="1"/><w:numFmt w:val="chineseCounting"/><w:lvlText w:val="（%2）"/><w:lvlJc w:val="left"/></w:lvl><w:lvl w:ilvl="2"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%3."/><w:lvlJc w:val="left"/></w:lvl></w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num></w:numbering>"#;
