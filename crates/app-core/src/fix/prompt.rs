//! Prompt for rewriting the paragraphs under a comment.
//!
//! Required parts (instructions, comment, paragraphs) always go in; the
//! optional parts are added in priority order while they fit the budget:
//! reviewer profile, neighbouring paragraphs, knowledge-base passages, then
//! the reviewer's past accepted fixes.

use super::context::{FixInput, FixMode};

#[derive(Debug, Clone)]
pub struct Passage {
    /// Citation number shown to the model, from 1.
    pub n: usize,
    pub title: String,
    pub heading_path: Vec<String>,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct Example {
    pub comment: String,
    pub original: String,
    pub revised: String,
}

pub struct PromptInput<'a> {
    pub input: &'a FixInput,
    pub reviewer: Option<&'a str>,
    /// Distilled reviewer profile, as plain text.
    pub profile: Option<&'a str>,
    pub passages: &'a [Passage],
    pub examples: &'a [Example],
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Included {
    pub profile: bool,
    pub neighbours: bool,
    pub passages: usize,
    pub examples: usize,
}

pub const SYSTEM: &str = "你是资深的党政机关公文和项目报告修改专家。用户会给你一条审稿专家的批注，以及批注所在的段落。你的任务是按批注意见修改这些段落，让报告更容易通过评审。

修改规则：
1. 只改批注指出的问题，其余文字保持原样，改动越小越好；不要顺手润色无关句子。
2. 输入几段就输出几段，顺序一致；不得合并、拆分、增加或删除段落。
3. 形如 ⟦图⟧、⟦注1⟧、⟦公式⟧ 的占位符代表图片、脚注和公式，必须原样保留在原来的位置。
4. 不得编造数据、文号、政策文件名称或事实。批注要求补充而原文和参考资料都没有依据的内容，用“【待补充：需要补充什么】”标出。
5. 用到参考资料时，在 citations 中列出资料编号；没用到就给空数组。
6. 语言符合公文规范：准确、简明、庄重，用语规范，数字、单位、标点符合国家标准。
7. 如果提供了审稿人画像和以往示例，按该审稿人的关注点和偏好来改。
8. 如果给出了“用户给出的修改方向”，按修改方向改，它与第 1 条冲突时以修改方向为准；但仍须遵守第 2、3、4 条。

只输出一个 JSON 对象，不要输出其他文字：
{\"paragraphs\": [\"修改后的第1段\", \"修改后的第2段\"], \"explanation\": \"一两句话说明改了什么、为什么\", \"citations\": [1]}";

pub const REWRITE_SYSTEM: &str = "你是资深的党政机关公文和项目报告修改专家。用户认为下面这些段落的原文本身就有问题（事实、逻辑、表述或结构不对），需要重写，而不只是按批注做小改。你的任务是结合批注、修改方向和上下文，把这些段落重写成正确、完整、符合公文规范的文字。

重写规则：
1. 可以调整句子结构、顺序和表述，删去错误或多余的内容，不必拘泥于原文措辞。
2. 输入几段就输出几段，顺序一致；不得合并、拆分、增加或删除段落。
3. 形如 ⟦图⟧、⟦注1⟧、⟦公式⟧ 的占位符代表图片、脚注和公式，必须原样保留。
4. 不得编造数据、文号、政策文件名称或事实。需要而原文和参考资料都没有依据的内容，用“【待补充：需要补充什么】”标出。
5. 用到参考资料时，在 citations 中列出资料编号；没用到就给空数组。
6. 语言符合公文规范：准确、简明、庄重，用语规范，数字、单位、标点符合国家标准。
7. 如果给出了“用户给出的修改方向”，以修改方向为准；但仍须遵守第 2、3、4 条。

只输出一个 JSON 对象，不要输出其他文字：
{\"paragraphs\": [\"重写后的第1段\", \"重写后的第2段\"], \"explanation\": \"一两句话说明重写了什么、为什么\", \"citations\": [1]}";

/// The system prompt for a mode.
pub fn system(mode: FixMode) -> &'static str {
    match mode {
        FixMode::Fix => SYSTEM,
        FixMode::Rewrite => REWRITE_SYSTEM,
    }
}

/// Rough token count: one per CJK character, one per four other characters.
pub fn estimate_tokens(s: &str) -> usize {
    let (mut cjk, mut other) = (0usize, 0usize);
    for c in s.chars() {
        if (c as u32) >= 0x2E80 {
            cjk += 1;
        } else {
            other += 1;
        }
    }
    cjk + other.div_ceil(4)
}

fn section(title: &str, body: &str) -> String {
    format!("【{title}】\n{}\n\n", body.trim())
}

/// Builds the user message within `budget` tokens (system prompt included).
pub fn build(p: &PromptInput<'_>, budget: usize) -> (String, Included) {
    let input = p.input;
    let mut required = String::new();
    if let Some(direction) = input.direction.as_deref().filter(|d| !d.trim().is_empty()) {
        required.push_str(&section("用户给出的修改方向（优先遵循）", direction));
    }
    if let Some(r) = p.reviewer {
        required.push_str(&section("审稿人", r));
    }
    required.push_str(&section("批注", &input.comment));
    if !input.replies.is_empty() {
        let replies: Vec<String> = input
            .replies
            .iter()
            .map(|(a, t)| format!("{a}：{t}"))
            .collect();
        required.push_str(&section("批注下的回复", &replies.join("\n")));
    }
    if !input.quote.is_empty() {
        required.push_str(&section("批注所指的原文", &input.quote));
    }
    if let Some(selection) = &input.selection {
        required.push_str(&section(
            "用户选定的修改范围（审稿人的标注可能没选全，以这里为准）",
            selection,
        ));
    }
    if !input.heading_path.is_empty() {
        required.push_str(&section("所在章节", &input.heading_path.join(" > ")));
    }
    let paragraphs: Vec<String> = input
        .paragraphs
        .iter()
        .enumerate()
        .map(|(i, (_, t))| format!("[第{}段] {t}", i + 1))
        .collect();
    let verb = match input.mode {
        FixMode::Fix => "修改",
        FixMode::Rewrite => "重写",
    };
    let target = section(
        &format!("需要{verb}的段落（共 {} 段）", paragraphs.len()),
        &paragraphs.join("\n"),
    );

    let mut used = estimate_tokens(system(input.mode))
        + estimate_tokens(&required)
        + estimate_tokens(&target)
        + 64;
    let mut included = Included::default();
    let fits = |text: &str, used: &mut usize| {
        let t = estimate_tokens(text);
        if *used + t <= budget {
            *used += t;
            true
        } else {
            false
        }
    };

    let mut profile = String::new();
    if let Some(text) = p.profile.filter(|t| !t.trim().is_empty()) {
        let s = section("审稿人画像（该审稿人一贯的关注点和偏好）", text);
        if fits(&s, &mut used) {
            profile = s;
            included.profile = true;
        }
    }

    let (mut before, mut after) = (String::new(), String::new());
    if !input.before.is_empty() || !input.after.is_empty() {
        let b = if input.before.is_empty() {
            String::new()
        } else {
            section("上文", &input.before.join("\n"))
        };
        let a = if input.after.is_empty() {
            String::new()
        } else {
            section("下文", &input.after.join("\n"))
        };
        if fits(&format!("{b}{a}"), &mut used) {
            (before, after) = (b, a);
            included.neighbours = true;
        }
    }

    let mut sources = String::new();
    for passage in p.passages {
        let place = if passage.heading_path.is_empty() {
            String::new()
        } else {
            format!(" {}", passage.heading_path.join(" > "))
        };
        let entry = format!(
            "[{}] 《{}》{place}\n{}\n",
            passage.n,
            passage.title,
            passage.text.trim()
        );
        if !fits(&entry, &mut used) {
            break;
        }
        sources.push_str(&entry);
        included.passages += 1;
    }

    let mut examples = String::new();
    for (i, ex) in p.examples.iter().enumerate() {
        let entry = format!(
            "示例{}：\n批注：{}\n原文：{}\n采纳的修改：{}\n",
            i + 1,
            ex.comment.trim(),
            ex.original.trim(),
            ex.revised.trim()
        );
        if !fits(&entry, &mut used) {
            break;
        }
        examples.push_str(&entry);
        included.examples += 1;
    }

    let mut message = String::new();
    message.push_str(&profile);
    message.push_str(&required);
    message.push_str(&before);
    message.push_str(&target);
    message.push_str(&after);
    if !sources.is_empty() {
        message.push_str(&section("参考资料（引用时写编号）", &sources));
    }
    if !examples.is_empty() {
        message.push_str(&section("该审稿人以往采纳的修改示例", &examples));
    }
    message.push_str("请按规则输出 JSON。");
    (message, included)
}

/// Follow-up message when the model's answer could not be used.
pub fn correction(problem: &str, paragraph_count: usize) -> String {
    format!(
        "上一次的输出无法使用：{problem}。请重新输出，只输出 JSON 对象，paragraphs 必须正好 {paragraph_count} 个字符串，占位符 ⟦…⟧ 原样保留。"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> FixInput {
        FixInput {
            comment_id: "1".into(),
            author: "张处长".into(),
            initials: String::new(),
            comment: "数据来源不明确".into(),
            replies: vec![],
            quote: "带动就业500人".into(),
            selection: None,
            paragraphs: vec![(3, "项目建成后预计带动就业500人。".into())],
            heading_path: vec!["一、项目概况".into(), "（二）建设效益".into()],
            before: vec!["上一段。".into()],
            after: vec!["下一段。".into()],
            mode: FixMode::Fix,
            direction: None,
        }
    }

    #[test]
    fn includes_optional_parts_while_they_fit() {
        let input = input();
        let passages: Vec<Passage> = (1..=5)
            .map(|n| Passage {
                n,
                title: format!("文件{n}"),
                heading_path: vec!["第三章".into()],
                text: "资料".repeat(200),
            })
            .collect();
        let examples = vec![Example {
            comment: "口径".into(),
            original: "原".into(),
            revised: "改".into(),
        }];
        let p = PromptInput {
            input: &input,
            reviewer: Some("张处长"),
            profile: Some("重视数据来源"),
            passages: &passages,
            examples: &examples,
        };
        let (all, inc) = build(&p, 100_000);
        assert_eq!(
            inc,
            Included {
                profile: true,
                neighbours: true,
                passages: 5,
                examples: 1
            }
        );
        assert!(all.contains("[第1段] 项目建成后预计带动就业500人。"));
        assert!(all.contains("一、项目概况 > （二）建设效益"));
        assert!(!all.contains("用户选定的修改范围"));

        let selected = FixInput {
            selection: Some("预计带动就业500人。".into()),
            ..input.clone()
        };
        let (with_selection, _) = build(
            &PromptInput {
                input: &selected,
                ..p
            },
            100_000,
        );
        assert!(with_selection.contains("用户选定的修改范围"));

        let (small, inc) = build(&p, estimate_tokens(SYSTEM) + 1_000);
        assert!(inc.passages < 5);
        assert!(
            small.contains("数据来源不明确"),
            "required parts always stay"
        );
    }

    #[test]
    fn direction_and_rewrite_mode() {
        let input = FixInput {
            mode: FixMode::Rewrite,
            direction: Some("改为按 2024 年统计口径表述".into()),
            ..input()
        };
        let p = PromptInput {
            input: &input,
            reviewer: None,
            profile: None,
            passages: &[],
            examples: &[],
        };
        let (msg, _) = build(&p, 100_000);
        assert!(msg.starts_with("【用户给出的修改方向（优先遵循）】\n改为按 2024 年统计口径表述"));
        assert!(msg.contains("需要重写的段落（共 1 段）"));
        assert_ne!(system(FixMode::Rewrite), system(FixMode::Fix));
    }

    #[test]
    fn estimates_tokens() {
        assert_eq!(estimate_tokens("中文"), 2);
        assert_eq!(estimate_tokens("abcdefgh"), 2);
    }
}
