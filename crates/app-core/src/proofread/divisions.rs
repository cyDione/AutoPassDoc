//! Built-in table of China's administrative divisions, used to spot a
//! project described with another place's name (张冠李戴): all
//! provincial-level divisions, all prefecture-level divisions (plus the
//! county-level cities governed directly by a province, such as 仙桃市), and
//! every district and county of the four municipalities.
//!
//! Each entry is a full name with an optional short form. Short forms that
//! are also ordinary words (金山银山, 朝阳产业, 日照时数) are kept for reading
//! the project's location but never flagged on their own.

use std::collections::HashMap;
use std::sync::LazyLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Level {
    Province,
    City,
    District,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Division {
    pub name: &'static str,
    /// Short form such as 崇明 for 崇明区; `None` when shorter than two
    /// characters or equal to the full name.
    pub short: Option<&'static str>,
    pub level: Level,
    /// The province-level division above (itself for a province).
    pub province: &'static str,
    /// The city above a district (a municipality for its districts); the
    /// city itself for a city; `None` for a province.
    pub city: Option<&'static str>,
    /// The short form is also an ordinary word and is only used when the
    /// project's location is read, never when flagging.
    pub ambiguous: bool,
}

impl Division {
    /// Whether `s` is this division's full name or short form.
    pub fn is_named(&self, s: &str) -> bool {
        self.name == s || self.short == Some(s)
    }
}

const MUNICIPALITIES: &[&str] = &["北京市", "天津市", "上海市", "重庆市"];

const PROVINCES: &str = "北京市|北京 天津市|天津 上海市|上海 重庆市|重庆 河北省|河北 山西省|山西 \
内蒙古自治区|内蒙古 辽宁省|辽宁 吉林省|吉林 黑龙江省|黑龙江 江苏省|江苏 浙江省|浙江 安徽省|安徽 \
福建省|福建 江西省|江西 山东省|山东 河南省|河南 湖北省|湖北 湖南省|湖南 广东省|广东 \
广西壮族自治区|广西 海南省|海南 四川省|四川 贵州省|贵州 云南省|云南 西藏自治区|西藏 陕西省|陕西 \
甘肃省|甘肃 青海省|青海 宁夏回族自治区|宁夏 新疆维吾尔自治区|新疆 台湾省|台湾 \
香港特别行政区|香港 澳门特别行政区|澳门";

/// Prefecture-level divisions by province; `全称|简称` where the short form
/// is not simply the name without 市/地区/盟.
const CITIES: &[(&str, &str)] = &[
    (
        "河北省",
        "石家庄市 唐山市 秦皇岛市 邯郸市 邢台市 保定市 张家口市 承德市 沧州市 廊坊市 衡水市",
    ),
    (
        "山西省",
        "太原市 大同市 阳泉市 长治市 晋城市 朔州市 晋中市 运城市 忻州市 临汾市 吕梁市",
    ),
    (
        "内蒙古自治区",
        "呼和浩特市 包头市 乌海市 赤峰市 通辽市 鄂尔多斯市 呼伦贝尔市 巴彦淖尔市 乌兰察布市 \
         兴安盟 锡林郭勒盟 阿拉善盟",
    ),
    (
        "辽宁省",
        "沈阳市 大连市 鞍山市 抚顺市 本溪市 丹东市 锦州市 营口市 阜新市 辽阳市 盘锦市 铁岭市 \
         朝阳市 葫芦岛市",
    ),
    (
        "吉林省",
        "长春市 吉林市 四平市 辽源市 通化市 白山市 松原市 白城市 延边朝鲜族自治州|延边",
    ),
    (
        "黑龙江省",
        "哈尔滨市 齐齐哈尔市 鸡西市 鹤岗市 双鸭山市 大庆市 伊春市 佳木斯市 七台河市 牡丹江市 \
         黑河市 绥化市 大兴安岭地区",
    ),
    (
        "江苏省",
        "南京市 无锡市 徐州市 常州市 苏州市 南通市 连云港市 淮安市 盐城市 扬州市 镇江市 泰州市 宿迁市",
    ),
    (
        "浙江省",
        "杭州市 宁波市 温州市 嘉兴市 湖州市 绍兴市 金华市 衢州市 舟山市 台州市 丽水市",
    ),
    (
        "安徽省",
        "合肥市 芜湖市 蚌埠市 淮南市 马鞍山市 淮北市 铜陵市 安庆市 黄山市 滁州市 阜阳市 宿州市 \
         六安市 亳州市 池州市 宣城市",
    ),
    (
        "福建省",
        "福州市 厦门市 莆田市 三明市 泉州市 漳州市 南平市 龙岩市 宁德市",
    ),
    (
        "江西省",
        "南昌市 景德镇市 萍乡市 九江市 新余市 鹰潭市 赣州市 吉安市 宜春市 抚州市 上饶市",
    ),
    (
        "山东省",
        "济南市 青岛市 淄博市 枣庄市 东营市 烟台市 潍坊市 济宁市 泰安市 威海市 日照市 临沂市 \
         德州市 聊城市 滨州市 菏泽市",
    ),
    (
        "河南省",
        "郑州市 开封市 洛阳市 平顶山市 安阳市 鹤壁市 新乡市 焦作市 濮阳市 许昌市 漯河市 三门峡市 \
         南阳市 商丘市 信阳市 周口市 驻马店市 济源市",
    ),
    (
        "湖北省",
        "武汉市 黄石市 十堰市 宜昌市 襄阳市 鄂州市 荆门市 孝感市 荆州市 黄冈市 咸宁市 随州市 \
         恩施土家族苗族自治州|恩施 仙桃市 潜江市 天门市 神农架林区|神农架",
    ),
    (
        "湖南省",
        "长沙市 株洲市 湘潭市 衡阳市 邵阳市 岳阳市 常德市 张家界市 益阳市 郴州市 永州市 怀化市 \
         娄底市 湘西土家族苗族自治州|湘西",
    ),
    (
        "广东省",
        "广州市 韶关市 深圳市 珠海市 汕头市 佛山市 江门市 湛江市 茂名市 肇庆市 惠州市 梅州市 \
         汕尾市 河源市 阳江市 清远市 东莞市 中山市 潮州市 揭阳市 云浮市",
    ),
    (
        "广西壮族自治区",
        "南宁市 柳州市 桂林市 梧州市 北海市 防城港市 钦州市 贵港市 玉林市 百色市 贺州市 河池市 \
         来宾市 崇左市",
    ),
    ("海南省", "海口市 三亚市 三沙市 儋州市"),
    (
        "四川省",
        "成都市 自贡市 攀枝花市 泸州市 德阳市 绵阳市 广元市 遂宁市 内江市 乐山市 南充市 眉山市 \
         宜宾市 广安市 达州市 雅安市 巴中市 资阳市 阿坝藏族羌族自治州|阿坝 甘孜藏族自治州|甘孜 \
         凉山彝族自治州|凉山",
    ),
    (
        "贵州省",
        "贵阳市 六盘水市 遵义市 安顺市 毕节市 铜仁市 黔西南布依族苗族自治州|黔西南 \
         黔东南苗族侗族自治州|黔东南 黔南布依族苗族自治州|黔南",
    ),
    (
        "云南省",
        "昆明市 曲靖市 玉溪市 保山市 昭通市 丽江市 普洱市 临沧市 楚雄彝族自治州|楚雄 \
         红河哈尼族彝族自治州|红河 文山壮族苗族自治州|文山 西双版纳傣族自治州|西双版纳 \
         大理白族自治州|大理 德宏傣族景颇族自治州|德宏 怒江傈僳族自治州|怒江 迪庆藏族自治州|迪庆",
    ),
    (
        "西藏自治区",
        "拉萨市 日喀则市 昌都市 林芝市 山南市 那曲市 阿里地区",
    ),
    (
        "陕西省",
        "西安市 铜川市 宝鸡市 咸阳市 渭南市 延安市 汉中市 榆林市 安康市 商洛市",
    ),
    (
        "甘肃省",
        "兰州市 嘉峪关市 金昌市 白银市 天水市 武威市 张掖市 平凉市 酒泉市 庆阳市 定西市 陇南市 \
         临夏回族自治州|临夏 甘南藏族自治州|甘南",
    ),
    (
        "青海省",
        "西宁市 海东市 海北藏族自治州|海北 黄南藏族自治州|黄南 海南藏族自治州|海南 \
         果洛藏族自治州|果洛 玉树藏族自治州|玉树 海西蒙古族藏族自治州|海西",
    ),
    ("宁夏回族自治区", "银川市 石嘴山市 吴忠市 固原市 中卫市"),
    (
        "新疆维吾尔自治区",
        "乌鲁木齐市 克拉玛依市 吐鲁番市 哈密市 昌吉回族自治州|昌吉 博尔塔拉蒙古自治州|博尔塔拉 \
         巴音郭楞蒙古自治州|巴音郭楞 阿克苏地区 克孜勒苏柯尔克孜自治州|克孜勒苏 喀什地区 和田地区 \
         伊犁哈萨克自治州|伊犁 塔城地区 阿勒泰地区",
    ),
];

/// Districts and counties of the four municipalities.
const DISTRICTS: &[(&str, &str)] = &[
    (
        "北京市",
        "东城区 西城区 朝阳区 丰台区 石景山区 海淀区 门头沟区 房山区 通州区 顺义区 昌平区 大兴区 \
         怀柔区 平谷区 密云区 延庆区",
    ),
    (
        "天津市",
        "和平区 河东区 河西区 南开区 河北区 红桥区 东丽区 西青区 津南区 北辰区 武清区 宝坻区 \
         滨海新区 宁河区 静海区 蓟州区",
    ),
    (
        "上海市",
        "黄浦区 徐汇区 长宁区 静安区 普陀区 虹口区 杨浦区 闵行区 宝山区 嘉定区 浦东新区 金山区 \
         松江区 青浦区 奉贤区 崇明区",
    ),
    (
        "重庆市",
        "万州区 涪陵区 渝中区 大渡口区 江北区 沙坪坝区 九龙坡区 南岸区 北碚区 綦江区 大足区 渝北区 \
         巴南区 黔江区 长寿区 江津区 合川区 永川区 南川区 璧山区 铜梁区 潼南区 荣昌区 开州区 \
         梁平区 武隆区 城口县 丰都县 垫江县 忠县 云阳县 奉节县 巫山县 巫溪县 \
         石柱土家族自治县|石柱 秀山土家族苗族自治县|秀山 酉阳土家族苗族自治县|酉阳 \
         彭水苗族土家族自治县|彭水",
    ),
];

/// Short forms that are also ordinary words, other provinces, rivers or
/// mountains: 金山银山、朝阳产业、大兴调查研究、日照时数、长治久安、黄山……
const AMBIGUOUS: &[&str] = &[
    "北京",
    "天津",
    "上海",
    "重庆",
    "吉林",
    "海南",
    "河北",
    "朝阳",
    "白山",
    "黄山",
    "大庆",
    "长治",
    "四平",
    "北海",
    "白银",
    "中山",
    "红河",
    "怒江",
    "玉树",
    "三明",
    "安康",
    "大同",
    "来宾",
    "河源",
    "日照",
    "阿里",
    "哈密",
    "和田",
    "普洱",
    "攀枝花",
    "牡丹江",
    "内江",
    "乐山",
    "兴安",
    "大兴安岭",
    "宁德",
    "包头",
    "海北",
    "海西",
    "黄南",
    "文山",
    "大理",
    "保山",
    "天水",
    "黑河",
    "山南",
    "海东",
    "湘西",
    "神农架",
    "天门",
    "金山",
    "大兴",
    "怀柔",
    "和平",
    "河东",
    "河西",
    "长寿",
    "滨海",
    "南岸",
    "江北",
    "东城",
    "西城",
    "宝山",
    "普陀",
    "南开",
    "北辰",
    "红桥",
    "东丽",
    "西青",
    "宁河",
    "静海",
    "巫山",
    "石柱",
    "秀山",
    "大足",
    "通州",
    "丰都",
    "云阳",
    "甘南",
    "阿坝",
    "潜江",
    "仙桃",
    "中卫",
    "吴忠",
    "德州",
    "泰安",
    "吉安",
    "抚州",
    "永州",
    "常德",
    "宜春",
    "安顺",
    "定西",
    "巴中",
    "广安",
    "雅安",
    "眉山",
    "金昌",
    "东营",
    "新余",
    "萍乡",
    "九江",
    "鹰潭",
];

fn default_short(name: &'static str) -> Option<&'static str> {
    for suffix in ["新区", "地区", "林区", "市", "盟", "区", "县", "省"] {
        if let Some(s) = name.strip_suffix(suffix) {
            return (s.chars().count() >= 2).then_some(s);
        }
    }
    None
}

fn entry(raw: &'static str) -> (&'static str, Option<&'static str>) {
    match raw.split_once('|') {
        Some((name, short)) => (name, Some(short)),
        None => (raw, default_short(raw)),
    }
}

static TABLE: LazyLock<Vec<Division>> = LazyLock::new(|| {
    let mut out = Vec::new();
    for raw in PROVINCES.split_whitespace() {
        let (name, short) = entry(raw);
        out.push(Division {
            name,
            short,
            level: Level::Province,
            province: name,
            city: MUNICIPALITIES.contains(&name).then_some(name),
            ambiguous: short.is_some_and(|s| AMBIGUOUS.contains(&s)),
        });
    }
    for (province, list) in CITIES {
        for raw in list.split_whitespace() {
            let (name, short) = entry(raw);
            out.push(Division {
                name,
                short,
                level: Level::City,
                province,
                city: Some(name),
                ambiguous: short.is_some_and(|s| AMBIGUOUS.contains(&s)),
            });
        }
    }
    for (city, list) in DISTRICTS {
        for raw in list.split_whitespace() {
            let (name, short) = entry(raw);
            out.push(Division {
                name,
                short,
                level: Level::District,
                province: city,
                city: Some(city),
                ambiguous: short.is_some_and(|s| AMBIGUOUS.contains(&s)),
            });
        }
    }
    out
});

static BY_NAME: LazyLock<HashMap<&'static str, Vec<usize>>> = LazyLock::new(|| {
    let mut map: HashMap<&'static str, Vec<usize>> = HashMap::new();
    for (i, d) in TABLE.iter().enumerate() {
        map.entry(d.name).or_default().push(i);
        if let Some(s) = d.short {
            map.entry(s).or_default().push(i);
        }
    }
    map
});

static FIRST_CHARS: LazyLock<std::collections::HashSet<char>> =
    LazyLock::new(|| BY_NAME.keys().filter_map(|k| k.chars().next()).collect());

/// Every division in the table.
pub fn all() -> &'static [Division] {
    &TABLE
}

/// Divisions called `name` (full name or short form), most specific level
/// first: 吉林 is both a province and a city.
pub fn lookup(name: &str) -> Vec<&'static Division> {
    let mut found: Vec<&Division> = BY_NAME
        .get(name)
        .map(|v| v.iter().map(|&i| &TABLE[i]).collect())
        .unwrap_or_default();
    // A full-name match beats a short-form one; deeper levels come first.
    found.sort_by_key(|d| (d.name != name, std::cmp::Reverse(d.level)));
    found
}

/// The division at `level` called `name`.
pub fn find(name: &str, level: Level) -> Option<&'static Division> {
    lookup(name).into_iter().find(|d| d.level == level)
}

/// Other divisions at the same level under the same parent.
pub fn siblings(d: &Division) -> impl Iterator<Item = &'static Division> + '_ {
    TABLE.iter().filter(move |o| {
        o.level == d.level
            && o.name != d.name
            && match d.level {
                Level::Province => true,
                Level::City => o.province == d.province,
                Level::District => o.city == d.city,
            }
    })
}

/// Every name (full and short) in `text`, longest first at each position,
/// without overlaps: (char start, char end, division).
pub fn scan(text: &str) -> Vec<(usize, usize, &'static Division)> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let mut hit = None;
        if !FIRST_CHARS.contains(&chars[i]) {
            i += 1;
            continue;
        }
        // Longest names in the table are under 16 characters.
        for len in (2..=16.min(chars.len() - i)).rev() {
            let s: String = chars[i..i + len].iter().collect();
            if let Some(&d) = lookup(&s).first() {
                hit = Some((len, d));
                break;
            }
        }
        match hit {
            Some((len, d)) => {
                out.push((i, i + len, d));
                i += len;
            }
            None => i += 1,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_complete_enough() {
        let count = |l| all().iter().filter(|d| d.level == l).count();
        assert_eq!(count(Level::Province), 34);
        assert!(count(Level::City) >= 333, "{}", count(Level::City));
        let districts = |c: &str| {
            all()
                .iter()
                .filter(|d| d.level == Level::District && d.city == Some(c))
                .count()
        };
        assert_eq!(districts("上海市"), 16);
        assert_eq!(districts("北京市"), 16);
        assert_eq!(districts("天津市"), 16);
        assert_eq!(districts("重庆市"), 38);
    }

    #[test]
    fn short_forms_and_lookups() {
        let d = find("崇明", Level::District).unwrap();
        assert_eq!(d.name, "崇明区");
        assert_eq!(d.city, Some("上海市"));
        assert_eq!(
            find("浦东新区", Level::District).unwrap().short,
            Some("浦东")
        );
        assert_eq!(find("延边", Level::City).unwrap().province, "吉林省");
        assert_eq!(
            find("石柱", Level::District).unwrap().name,
            "石柱土家族自治县"
        );
        assert_eq!(find("忠县", Level::District).unwrap().short, None);
        assert!(find("金山区", Level::District).unwrap().ambiguous);
        assert!(!find("嘉定区", Level::District).unwrap().ambiguous);
        let jilin = lookup("吉林");
        assert_eq!(jilin[0].level, Level::City);
        assert_eq!(jilin[1].level, Level::Province);
        assert_eq!(find("苏州", Level::City).unwrap().province, "江苏省");
    }

    #[test]
    fn siblings_share_the_parent() {
        let d = find("崇明区", Level::District).unwrap();
        let s: Vec<_> = siblings(d).map(|d| d.name).collect();
        assert_eq!(s.len(), 15);
        assert!(s.contains(&"浦东新区"));
        let c = find("苏州市", Level::City).unwrap();
        assert!(siblings(c).all(|d| d.province == "江苏省"));
        assert_eq!(siblings(c).count(), 12);
    }

    #[test]
    fn scans_longest_names() {
        let hits = scan("上海市浦东新区与崇明区");
        let names: Vec<_> = hits.iter().map(|h| h.2.name).collect();
        assert_eq!(names, ["上海市", "浦东新区", "崇明区"]);
        assert_eq!((hits[1].0, hits[1].1), (3, 7));
    }
}
