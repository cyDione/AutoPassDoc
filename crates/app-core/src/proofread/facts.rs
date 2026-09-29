//! Reading the project's key facts (name, place, owner, investment) from
//! the document, and resolving the place against the division table.

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;

use super::divisions::{self, Division, Level};
use super::rules;
use super::{ProjectFacts, ProofParagraph};

static NAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(?:项目|工程)名称\s*[:：]\s*(.+?)\s*[。；;]?\s*$").unwrap());
static LOCATION_LABEL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:建设地点|项目地点|项目选址|建设地址|项目地址)\s*[:：]\s*([^。；;\n]+)").unwrap()
});
static LOCATION_VERB: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?:项目|工程|场地|地块)(?:拟)?(?:位于|选址于|选址位于|地处|坐落于|建设地点位于|建于|选址在)([^，,。；;]+)",
    )
    .unwrap()
});
static OWNER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:建设单位|项目单位|项目法人|业主单位|项目业主|项目建设单位)\s*[:：为]\s*([^。；;，,\n]+)")
        .unwrap()
});
static INVESTMENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"总投资(?:估算|概算)?(?:额)?(?:为|约|约为|共计|合计|:|：|\s)*([0-9][0-9,.]*\s*(?:亿元|万元|元))")
        .unwrap()
});
static PERIOD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:建设工期|建设周期|计划工期|施工工期)(?:为|约|:|：|\s)*([^，,。；;]{1,20})")
        .unwrap()
});
static SCALE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"建设规模(?:为|:|：|\s)*([^。；;]{2,60})").unwrap());

/// Report types dropped from the title to get the project's name.
const TITLE_SUFFIXES: &[&str] = &[
    "可行性研究报告",
    "项目建议书",
    "初步设计说明书",
    "初步设计",
    "项目申请报告",
    "资金申请报告",
    "实施方案",
    "研究报告",
    "申请报告",
    "设计说明",
    "报告",
];

fn clean_title(title: &str) -> String {
    let mut t = title.trim().to_string();
    // Drop a trailing （送审稿） / (征求意见稿).
    for (open, close) in [('（', '）'), ('(', ')')] {
        if t.ends_with(close)
            && let Some(i) = t.rfind(open)
        {
            t.truncate(i);
        }
    }
    let t = t.trim();
    for s in TITLE_SUFFIXES {
        if let Some(rest) = t.strip_suffix(s)
            && rest.chars().count() >= 4
        {
            return rest.trim().to_string();
        }
    }
    t.to_string()
}

/// The document's title: the first short, unnumbered paragraph near the
/// top that is not a sentence or a label like 目录.
fn title(paras: &[ProofParagraph]) -> Option<&str> {
    paras
        .iter()
        .filter(|p| !p.text.trim().is_empty())
        .take(20)
        .map(|p| (p, p.text.trim()))
        .find(|(p, t)| {
            let n = t.chars().count();
            (4..=60).contains(&n)
                && !matches!(*t, "目录" | "目 录" | "前言" | "说明")
                && !t.ends_with('。')
                && p.list_label.is_none()
                && rules::parse_prefix(t, p.heading_level.is_some()).is_none()
        })
        .map(|(_, t)| t)
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Place {
    pub province: Option<&'static Division>,
    pub city: Option<&'static Division>,
    pub district: Option<&'static Division>,
}

impl Place {
    fn is_empty(&self) -> bool {
        self.province.is_none() && self.city.is_none() && self.district.is_none()
    }

    /// Fills the levels above the most specific one known.
    fn complete(mut self) -> Self {
        if let Some(d) = self.district {
            if self.city.is_none_or(|c| Some(c.name) != d.city) {
                self.city = d.city.and_then(city_named);
            }
            self.province = divisions::find(d.province, Level::Province);
        }
        if let Some(c) = self.city
            && self.province.is_none_or(|p| p.name != c.province)
        {
            self.province = divisions::find(c.province, Level::Province);
        }
        self
    }
}

/// A city, or a municipality acting as one.
fn city_named(name: &str) -> Option<&'static Division> {
    divisions::find(name, Level::City)
        .or_else(|| divisions::find(name, Level::Province).filter(|p| p.city == Some(p.name)))
}

/// Places named in `text`, first mention of each level.
fn places_in(text: &str) -> Place {
    let mut place = Place::default();
    for (_, _, d) in divisions::scan(text) {
        match d.level {
            Level::Province if place.province.is_none() => place.province = Some(d),
            Level::City if place.city.is_none() => place.city = Some(d),
            Level::District if place.district.is_none() => place.district = Some(d),
            _ => {}
        }
    }
    // A municipality is also its own city.
    if place.city.is_none() {
        place.city = place.province.filter(|p| p.city == Some(p.name));
    }
    place
}

/// Resolves the place in `facts` against the division table; unknown names
/// stay `None`.
pub fn resolve(facts: &ProjectFacts) -> Place {
    fn get(v: &Option<String>) -> Option<&str> {
        v.as_deref().map(str::trim).filter(|v| !v.is_empty())
    }
    let place = Place {
        province: get(&facts.province).and_then(|n| divisions::find(n, Level::Province)),
        city: get(&facts.city).and_then(city_named),
        district: get(&facts.district).and_then(|n| divisions::find(n, Level::District)),
    };
    place.complete()
}

fn first_capture<'a>(re: &Regex, paras: &'a [&'a ProofParagraph]) -> Option<String> {
    paras.iter().find_map(|p| {
        re.captures(&p.text)
            .map(|c| c[1].trim().to_string())
            .filter(|s| !s.is_empty())
    })
}

/// Reads the project's facts from the document: title, labelled fields
/// such as 项目名称 / 建设地点 / 建设单位, sentences like 项目位于……, and
/// failing those the district named most often near the top.
pub fn extract_facts(paras: &[ProofParagraph]) -> ProjectFacts {
    let texts: Vec<&ProofParagraph> = paras.iter().filter(|p| !p.text.trim().is_empty()).collect();
    let title = title(paras);
    let name = first_capture(&NAME, &texts)
        .map(|n| clean_title(&n))
        .or_else(|| title.map(clean_title))
        .filter(|n| !n.is_empty());

    let mut place = Place::default();
    let mut located = Vec::new();
    for re in [&*LOCATION_LABEL, &*LOCATION_VERB] {
        located.extend(
            texts
                .iter()
                .filter_map(|p| re.captures(&p.text))
                .map(|c| c[1].to_string()),
        );
    }
    if let Some(n) = &name {
        located.push(n.clone());
    }
    for text in &located {
        let found = places_in(text);
        place.district = place.district.or(found.district);
        place.city = place.city.or(found.city);
        place.province = place.province.or(found.province);
        if place.district.is_some() {
            break;
        }
    }
    if place.district.is_none() {
        // The district named most often in the first 15% of the text.
        let head = (texts.len() * 15).div_ceil(100).max(10).min(texts.len());
        let mut counts: HashMap<&'static str, (usize, usize)> = HashMap::new();
        for (order, p) in texts[..head].iter().enumerate() {
            for (_, _, d) in divisions::scan(&p.text) {
                if d.level == Level::District || (d.level == Level::City && place.city.is_none()) {
                    let e = counts.entry(d.name).or_insert((0, order));
                    e.0 += 1;
                }
            }
        }
        let best = counts
            .iter()
            .filter(|(_, (n, _))| *n >= 2)
            .max_by_key(|(name, (n, first))| {
                let district = divisions::find(name, Level::District).is_some();
                (district, *n, std::cmp::Reverse(*first))
            })
            .and_then(|(name, _)| {
                divisions::find(name, Level::District)
                    .or_else(|| divisions::find(name, Level::City))
            });
        if let Some(d) = best {
            match d.level {
                Level::District => place.district = Some(d),
                _ => place.city = Some(d),
            }
        }
    }
    let place = if place.is_empty() {
        place
    } else {
        place.complete()
    };

    let mut others = Vec::new();
    if let Some(v) = first_capture(&INVESTMENT, &texts) {
        others.push(("总投资".to_string(), v.replace(' ', "")));
    }
    if let Some(v) = first_capture(&SCALE, &texts) {
        others.push(("建设规模".to_string(), v));
    }
    if let Some(v) = first_capture(&PERIOD, &texts) {
        others.push(("建设工期".to_string(), v));
    }
    ProjectFacts {
        name,
        province: place.province.map(|d| d.name.to_string()),
        city: place.city.map(|d| d.name.to_string()),
        district: place.district.map(|d| d.name.to_string()),
        owner: first_capture(&OWNER, &texts),
        others,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paras(texts: &[&str]) -> Vec<ProofParagraph> {
        texts
            .iter()
            .enumerate()
            .map(|(i, t)| ProofParagraph::new(i, *t))
            .collect()
    }

    #[test]
    fn reads_labelled_fields() {
        let f = extract_facts(&paras(&[
            "崇明区城桥镇污水管网改造工程可行性研究报告（送审稿）",
            "第一章 总论",
            "项目名称：崇明区城桥镇污水管网改造工程",
            "建设地点：上海市崇明区城桥镇",
            "建设单位：上海市崇明区水务局",
            "项目总投资约 3.2亿元，其中工程费用2.6亿元。",
            "建设工期为24个月。",
        ]));
        assert_eq!(f.name.as_deref(), Some("崇明区城桥镇污水管网改造工程"));
        assert_eq!(f.province.as_deref(), Some("上海市"));
        assert_eq!(f.city.as_deref(), Some("上海市"));
        assert_eq!(f.district.as_deref(), Some("崇明区"));
        assert_eq!(f.owner.as_deref(), Some("上海市崇明区水务局"));
        assert_eq!(
            f.others,
            [
                ("总投资".to_string(), "3.2亿元".to_string()),
                ("建设工期".to_string(), "24个月".to_string())
            ]
        );
    }

    #[test]
    fn reads_location_sentences_and_title() {
        let f = extract_facts(&paras(&[
            "吴中区太湖生态岸线修复工程实施方案",
            "本项目位于江苏省苏州市吴中区，东临太湖。",
        ]));
        assert_eq!(f.name.as_deref(), Some("吴中区太湖生态岸线修复工程"));
        assert_eq!(f.city.as_deref(), Some("苏州市"));
        assert_eq!(f.province.as_deref(), Some("江苏省"));
        assert_eq!(f.district, None, "吴中区 is not in the table");
    }

    #[test]
    fn falls_back_to_the_most_mentioned_district() {
        let f = extract_facts(&paras(&[
            "污水管网改造工程可行性研究报告",
            "近年来，嘉定区大力推进水环境治理。",
            "嘉定区现有污水管网约 1200 公里。",
            "参考浦东新区的做法。",
        ]));
        assert_eq!(f.district.as_deref(), Some("嘉定区"));
        assert_eq!(f.city.as_deref(), Some("上海市"));
        assert_eq!(f.name.as_deref(), Some("污水管网改造工程"));
    }

    #[test]
    fn resolves_short_and_full_names() {
        let p = resolve(&ProjectFacts {
            district: Some("崇明".into()),
            ..Default::default()
        });
        assert_eq!(p.district.unwrap().name, "崇明区");
        assert_eq!(p.city.unwrap().name, "上海市");
        assert_eq!(p.province.unwrap().name, "上海市");
        let p = resolve(&ProjectFacts {
            city: Some("无锡".into()),
            ..Default::default()
        });
        assert_eq!(p.province.unwrap().name, "江苏省");
        assert!(resolve(&ProjectFacts::default()).is_empty());
    }
}
