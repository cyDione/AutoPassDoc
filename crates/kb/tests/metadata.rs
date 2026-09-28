mod common;

use kb::DocMeta;
use kb::metadata::{extract_metadata, normalize_doc_number, parse_date};

fn meta(text: &str) -> DocMeta {
    extract_metadata(&text.lines().collect::<Vec<_>>())
}

fn some(s: &str) -> Option<String> {
    Some(s.to_string())
}

#[test]
fn red_header_notice() {
    let m = meta(common::SAMPLE);
    assert_eq!(
        m.title,
        some("某某市人民政府关于印发《某某市公共数据管理办法》的通知")
    );
    assert_eq!(m.doc_number, some("某政发〔2024〕7号"));
    assert_eq!(m.issuer, some("某某市人民政府"));
    assert_eq!(m.date, some("2024-03-15"));
}

#[test]
fn web_version_with_chinese_numeral_date() {
    let m = meta(
        "国务院办公厅关于进一步优化政务服务提升行政效能推动“高效办成一件事”的指导意见
国办发〔2024〕3号
各省、自治区、直辖市人民政府，国务院各部委、各直属机构：
　　为深入贯彻落实党中央、国务院决策部署，经国务院同意，现提出如下意见。
　　一、总体要求
　　坚持以人民为中心的发展思想，自2024年3月1日起在全国推行。
国务院办公厅
二〇二四年一月十二日
（本文有删减）",
    );
    assert_eq!(
        m.title,
        some("国务院办公厅关于进一步优化政务服务提升行政效能推动“高效办成一件事”的指导意见")
    );
    assert_eq!(m.doc_number, some("国办发〔2024〕3号"));
    assert_eq!(m.issuer, some("国务院办公厅"));
    assert_eq!(m.date, some("2024-01-12"));
}

#[test]
fn square_brackets_and_issuer_on_the_date_line() {
    let m = meta(
        "北京市人民政府
京政发[2023]5号
北京市人民政府关于加快建设全球数字经济标杆城市的实施方案
各区人民政府，市政府各委、办、局：
　　现将有关事项通知如下。
北京市人民政府　　2023年5月10日",
    );
    assert_eq!(m.doc_number, some("京政发〔2023〕5号"));
    assert_eq!(
        m.title,
        some("北京市人民政府关于加快建设全球数字经济标杆城市的实施方案")
    );
    assert_eq!(m.issuer, some("北京市人民政府"));
    assert_eq!(m.date, some("2023-05-10"));
}

#[test]
fn joint_issuers_and_printing_date_in_the_footer() {
    let m = meta(
        "财政部 税务总局文件
财预【2022】1号
关于进一步实施小微企业所得税优惠政策的公告
　　为进一步支持小微企业发展，现将有关税收政策公告如下。
　　本公告执行期限为2022年1月1日至2024年12月31日。
财政部　　　　税务总局
（盖章）
2022年3月14日
抄送：各省、自治区、直辖市财政厅（局）。
财政部办公厅　　　　2022年3月15日印发",
    );
    assert_eq!(m.doc_number, some("财预〔2022〕1号"));
    assert_eq!(m.title, some("关于进一步实施小微企业所得税优惠政策的公告"));
    assert_eq!(m.issuer, some("财政部 税务总局"));
    assert_eq!(m.date, some("2022-03-14"));
}

#[test]
fn law_with_adoption_date_under_the_title() {
    let m = meta(
        "中华人民共和国数据安全法
（2021年6月10日第十三届全国人民代表大会常务委员会第二十九次会议通过）
第一章　总　　则
第一条　为了规范数据处理活动，保障数据安全，促进数据开发利用，制定本法。
第五十五条　本法自2021年9月1日起施行。",
    );
    assert_eq!(m.title, some("中华人民共和国数据安全法"));
    assert_eq!(m.date, some("2021-06-10"));
    assert_eq!(m.doc_number, None);
    assert_eq!(m.issuer, None);
}

#[test]
fn company_notice_with_circle_zero_and_wrapped_title() {
    let m = meta(
        "某某科技集团有限公司关于印发《2024年度安全生产
工作要点》的通知
各部门、各子公司：
　　现将《2024年度安全生产工作要点》印发给你们，请遵照执行。
某某科技集团有限公司
二○二四年三月五日",
    );
    assert_eq!(
        m.title,
        some("某某科技集团有限公司关于印发《2024年度安全生产工作要点》的通知")
    );
    assert_eq!(m.issuer, some("某某科技集团有限公司"));
    assert_eq!(m.date, some("2024-03-05"));
    assert_eq!(m.doc_number, None);
}

#[test]
fn issuer_from_title_when_nothing_else() {
    let m = meta("教育部关于做好2024年普通高校招生工作的通知\n各省级招生考试机构：\n正文。");
    assert_eq!(m.issuer, some("教育部"));
    assert_eq!(m.date, None);
}

#[test]
fn normalizes_numbers_and_dates() {
    assert_eq!(
        normalize_doc_number("国办函(2024)101号").as_deref(),
        Some("国办函〔2024〕101号")
    );
    assert_eq!(
        normalize_doc_number("（国办发［2024］12号）").as_deref(),
        Some("国办发〔2024〕12号")
    );
    assert_eq!(
        parse_date("二〇二四年十月一日").as_deref(),
        Some("2024-10-01")
    );
    assert_eq!(
        parse_date("2024 年 1 月 31 日").as_deref(),
        Some("2024-01-31")
    );
}
