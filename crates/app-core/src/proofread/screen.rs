//! Cheap passes that run before the chat model: which paragraphs are worth
//! reading at all, typos a word list settles, and the key figures of the
//! document (so the model no longer has to list them).

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;

use super::model::Fact;
use super::rules::{find_chars, is_han};
use super::{Category, Issue, ProofParagraph, Severity};

/// Fewer Chinese characters than this: labels, numbers, units and short
/// table cells, which the model is not asked to read.
const MIN_HAN: usize = 3;

/// Whether the chat model should read `p`.
pub fn worth_reading(p: &ProofParagraph) -> bool {
    p.text.chars().filter(|&c| is_han(c)).count() >= MIN_HAN
}

/// Paragraphs to read, the first of each distinct text only, and for each
/// paragraph left out because an earlier one has the same text, that
/// earlier paragraph's index (`copy → original`).
pub fn distinct(paras: &[ProofParagraph]) -> (Vec<ProofParagraph>, Vec<(usize, usize)>) {
    let mut first: HashMap<&str, usize> = HashMap::new();
    let mut keep = Vec::new();
    let mut copies = Vec::new();
    for p in paras.iter().filter(|p| worth_reading(p)) {
        match first.get(p.text.trim()) {
            Some(&orig) => copies.push((p.index, orig)),
            None => {
                first.insert(p.text.trim(), p.index);
                keep.push(p.clone());
            }
        }
    }
    (keep, copies)
}

/// Forms that are wrong wherever they appear, with the right form. Only
/// words that cannot be part of a longer correct word are listed.
const WRONG_WORDS: &[(&str, &str)] = &[
    ("布署", "部署"),
    ("再接再励", "再接再厉"),
    ("按装", "安装"),
    ("重迭", "重叠"),
    ("松驰", "松弛"),
    ("针贬", "针砭"),
    ("一股作气", "一鼓作气"),
    ("莫明其妙", "莫名其妙"),
    ("按步就班", "按部就班"),
    ("迫不急待", "迫不及待"),
    ("默守成规", "墨守成规"),
    ("穿流不息", "川流不息"),
    ("变本加利", "变本加厉"),
    ("走头无路", "走投无路"),
    ("一愁莫展", "一筹莫展"),
    ("出奇不意", "出其不意"),
    ("名符其实", "名副其实"),
    ("美仑美奂", "美轮美奂"),
    ("一如继往", "一如既往"),
    ("再所难免", "在所难免"),
    ("不径而走", "不胫而走"),
    ("丰富多采", "丰富多彩"),
    ("励行节约", "厉行节约"),
    ("建立建全", "建立健全"),
    ("截止目前", "截至目前"),
    ("截止到目前", "截至目前"),
    ("渲泄", "宣泄"),
    ("幅射", "辐射"),
    ("复盖", "覆盖"),
    ("帐户", "账户"),
    ("帐号", "账号"),
    ("帐目", "账目"),
    ("报帐", "报账"),
    ("峻工", "竣工"),
    ("座落", "坐落"),
    ("防碍", "妨碍"),
    ("陷井", "陷阱"),
    ("急燥", "急躁"),
    ("烦燥", "烦躁"),
    ("冒然", "贸然"),
    ("寒喧", "寒暄"),
    ("必竟", "毕竟"),
    ("暴光", "曝光"),
    ("录象", "录像"),
    ("编缉", "编辑"),
    ("气慨", "气概"),
];

/// Typos from the word list.
pub fn check_typo_words(paras: &[ProofParagraph]) -> Vec<Issue> {
    let mut out = Vec::new();
    for p in paras {
        if !WRONG_WORDS.iter().any(|(w, _)| p.text.contains(w)) {
            continue;
        }
        let chars: Vec<char> = p.text.chars().collect();
        for (wrong, right) in WRONG_WORDS {
            let len = wrong.chars().count();
            let mut from = 0;
            while let Some(at) = find_chars(&chars[from..], wrong) {
                let start = from + at;
                out.push(Issue::span(
                    Category::Typo,
                    Severity::Error,
                    p.index,
                    &chars,
                    start,
                    start + len,
                    Some((*right).to_string()),
                    format!("“{wrong}”应为“{right}”"),
                ));
                from = start + len;
            }
        }
    }
    out
}

/// Metric names, longest first so the longest one matches. None starts
/// with 工程 or 项目: in 一期工程总投资 that belongs to the subject.
const METRICS: &[&str] = &[
    "静态总投资",
    "动态总投资",
    "估算总投资",
    "总建筑面积",
    "总用地面积",
    "工程费用",
    "建设投资",
    "总投资",
    "占地面积",
    "用地面积",
    "建筑面积",
    "绿化面积",
    "建设工期",
    "施工工期",
    "总工期",
    "工期",
    "管网长度",
    "管线长度",
    "道路全长",
    "总长度",
    "全长",
    "总长",
    "处理规模",
    "设计规模",
    "供水规模",
    "处理能力",
];

const UNITS: &[&str] = &[
    "亿元",
    "万元",
    "千元",
    "元",
    "万平方米",
    "平方公里",
    "平方千米",
    "平方米",
    "平米",
    "万㎡",
    "㎡",
    "m²",
    "m2",
    "公顷",
    "hm²",
    "亩",
    "公里",
    "千米",
    "km",
    "个月",
    "日历天",
    "天",
    "年",
    "万吨/日",
    "吨/日",
    "万m³/d",
    "m³/d",
    "立方米/日",
    "米",
    "m",
];

static FIGURE: LazyLock<Regex> = LazyLock::new(|| {
    let alt = |xs: &[&str]| {
        xs.iter()
            .map(|x| regex::escape(x))
            .collect::<Vec<_>>()
            .join("|")
    };
    Regex::new(&format!(
        r"(?P<metric>{})[^\d。；;，,、（）()]{{0,8}}?(?P<value>\d+(?:,\d{{3}})*(?:\.\d+)?)\s*(?P<unit>{})",
        alt(METRICS),
        alt(UNITS)
    ))
    .unwrap()
});

/// Endings of a phrase that names what a figure is about.
const SUBJECT_ENDS: &[&str] = &[
    "工程", "项目", "一期", "二期", "三期", "近期", "远期", "厂", "站", "管网", "道路", "桥梁",
    "中心", "园区", "片区",
];

/// What the figure before `at` is about: the phrase right before the
/// metric when it names a project part, otherwise the project.
fn subject_before(text: &str, at: usize) -> String {
    let before = &text[..at];
    let start = before
        .rfind(['。', '；', '，', '、', '：', ',', ';', ':', '（', '('])
        .map(|i| i + before[i..].chars().next().map_or(1, char::len_utf8))
        .unwrap_or(0);
    let mut phrase = before[start..].trim().trim_end_matches('的');
    for lead in ["其中", "另外", "此外", "经复核", "经测算", "根据测算"] {
        phrase = phrase.trim_start_matches(lead);
    }
    // Keep the last few characters: "位于崇明区的污水处理厂" → "污水处理厂".
    let chars: Vec<char> = phrase.chars().collect();
    let tail: String = chars[chars.len().saturating_sub(10)..].iter().collect();
    if SUBJECT_ENDS.iter().any(|e| tail.ends_with(e)) {
        tail
    } else {
        "项目".to_string()
    }
}

/// Key figures (investment, area, schedule, length, capacity) stated in
/// the text, for comparing them across the document.
pub fn extract_figures(paras: &[ProofParagraph]) -> Vec<Fact> {
    let mut out = Vec::new();
    for p in paras {
        for m in FIGURE.captures_iter(&p.text) {
            let (whole, metric) = (m.get(0).unwrap(), m.name("metric").unwrap());
            out.push(Fact {
                paragraph: p.index,
                subject: subject_before(&p.text, metric.start()),
                metric: metric.as_str().to_string(),
                value: m["value"].replace(',', ""),
                unit: m["unit"].to_string(),
                text: whole.as_str().to_string(),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(index: usize, text: &str) -> ProofParagraph {
        ProofParagraph::new(index, text)
    }

    #[test]
    fn short_and_repeated_paragraphs_are_read_once() {
        let paras = [
            p(0, "3.2"),
            p(1, "合计"),
            p(2, "本项目位于崇明区城桥镇。"),
            p(3, "注：单位为万元。"),
            p(4, "本项目位于崇明区城桥镇。"),
        ];
        let (keep, copies) = distinct(&paras);
        let kept: Vec<usize> = keep.iter().map(|p| p.index).collect();
        assert_eq!(kept, [2, 3]);
        assert_eq!(copies, [(4, 2)]);
    }

    #[test]
    fn word_list_finds_every_occurrence() {
        let issues = check_typo_words(&[p(3, "统一布署，再接再励，统一布署。截止目前已完成。")]);
        let found: Vec<_> = issues
            .iter()
            .map(|i| (i.start, i.original.as_str(), i.suggestion.as_deref()))
            .collect();
        assert_eq!(
            found,
            [
                (2, "布署", Some("部署")),
                (12, "布署", Some("部署")),
                (5, "再接再励", Some("再接再厉")),
                (15, "截止目前", Some("截至目前")),
            ]
        );
        assert!(check_typo_words(&[p(0, "统一部署，全面构建全方位体系。")]).is_empty());
    }

    #[test]
    fn figures_with_subject_metric_and_unit() {
        let facts = extract_figures(&[
            p(1, "本项目位于浦东新区，总投资3.2亿元，建设工期约24个月。"),
            p(2, "其中一期工程总投资为12,000万元，占地面积1.5公顷。"),
            p(3, "污水处理厂的处理规模为5万吨/日。"),
        ]);
        let got: Vec<_> = facts
            .iter()
            .map(|f| {
                (
                    f.paragraph,
                    f.subject.as_str(),
                    f.metric.as_str(),
                    f.value.as_str(),
                    f.unit.as_str(),
                    f.text.as_str(),
                )
            })
            .collect();
        assert_eq!(
            got,
            [
                (1, "项目", "总投资", "3.2", "亿元", "总投资3.2亿元"),
                (1, "项目", "建设工期", "24", "个月", "建设工期约24个月"),
                (
                    2,
                    "一期工程",
                    "总投资",
                    "12000",
                    "万元",
                    "总投资为12,000万元"
                ),
                (2, "项目", "占地面积", "1.5", "公顷", "占地面积1.5公顷"),
                (
                    3,
                    "污水处理厂",
                    "处理规模",
                    "5",
                    "万吨/日",
                    "处理规模为5万吨/日"
                ),
            ]
        );
    }
}
