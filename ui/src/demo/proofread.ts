import type { ProofCitation, ProofIssue, ProofOptions, ProofReport } from "../types";

/**
 * Demo proofreading over the sample report: a few real patterns found in its
 * text (a sentence repeated inside a paragraph, wordy "进行…" phrasing) and
 * the documents it cites. No model, no network.
 */
export function demoProofread(paragraphs: [number, string][], options: ProofOptions): Omit<ProofReport, "paragraphs"> & { paragraphs: Record<number, string> } {
  const wants = (c: ProofIssue["category"]) => options.categories.length === 0 || options.categories.includes(c);
  const issues: ProofIssue[] = [];
  const texts: Record<number, string> = {};
  const push = (index: number, text: string, issue: Omit<ProofIssue, "id" | "paragraph">) => {
    issues.push({ ...issue, paragraph: index, id: `${issue.category}-${index}-${issue.start}-${issue.end}` });
    texts[index] = text;
  };

  let repeats = 0;
  let wordy = 0;
  const citations = new Map<string, ProofCitation>();
  for (const [index, text] of paragraphs) {
    const chars = [...text];
    if (wants("consistency") && repeats < 12) {
      // A sentence written twice in the same paragraph.
      const sentences = text.match(/[^。；]+[。；]/g) ?? [];
      const seen = new Set<string>();
      let at = 0;
      for (const s of sentences) {
        const start = [...text.slice(0, text.indexOf(s, at))].length;
        at = text.indexOf(s, at) + s.length;
        if (s.length >= 12 && seen.has(s)) {
          push(index, text, {
            category: "consistency",
            severity: "warning",
            start,
            end: start + [...s].length,
            original: s,
            suggestion: "",
            reason: "同一段落中这句话出现了两次，建议删除重复的一句。",
            source: "model",
          });
          repeats++;
          break;
        }
        seen.add(s);
      }
    }
    const phrase = "通过标准接口进行数据交换";
    if (wants("typo") && wordy < 6 && text.includes(phrase)) {
      const start = [...text.slice(0, text.indexOf(phrase))].length;
      push(index, text, {
        category: "typo",
        severity: "warning",
        start,
        end: start + [...phrase].length,
        original: phrase,
        suggestion: "通过标准接口交换数据",
        reason: "“进行＋动词”结构冗余，建议精简。",
        source: "rule",
      });
      wordy++;
    }
    if (wants("format") && chars.length > 40 && /[。；：]$/.test(text) === false && !/^(重点任务|第\d|建设内容)/.test(text) && issues.length < 40) {
      push(index, text, {
        category: "format",
        severity: "warning",
        start: chars.length,
        end: chars.length,
        original: "",
        suggestion: null,
        reason: "正文段落末尾缺少标点。",
        source: "rule",
      });
    }
    for (const m of text.matchAll(/(GB\/T\s*[\d.]+-\d{4})?《([^》]{4,40})》/g)) {
      const title = m[2];
      const c = citations.get(title) ?? {
        title,
        kind: m[1] ? "standard" : title.includes("意见") ? "policy" : "unknown",
        standardNo: m[1]?.replace(/\s+/g, " ") ?? null,
        docNo: null,
        paragraphs: [],
        status: null,
      };
      if (!c.paragraphs.includes(index)) c.paragraphs.push(index);
      citations.set(title, c);
    }
  }

  const cited = [...citations.values()].filter((c) => c.paragraphs.length > 1);
  if (wants("citation"))
    for (const c of cited) {
      c.status = { status: "current", replacement: null, reason: "演示模式：未联网核查，按现行处理。" };
    }

  const counts: ProofReport["counts"] = {};
  for (const i of issues) counts[i.category] = (counts[i.category] ?? 0) + 1;
  return {
    facts: options.facts ?? {
      name: "某市数字政府项目",
      province: null,
      city: "某市",
      district: null,
      owner: "某市大数据管理局",
      others: [["建设周期", "2024–2026 年"]],
    },
    issues,
    citations: cited,
    counts,
    sections: Math.ceil(paragraphs.length / 60),
    cachedSections: 0,
    modelCalls: options.useModel ? Math.ceil(paragraphs.length / 60) : 0,
    skipped: 0,
    screenedOut: options.useModel && options.screen ? Math.floor(paragraphs.length * 0.7) : 0,
    screenCalls: options.useModel && options.screen ? Math.ceil(paragraphs.length / 12) : 0,
    failures: [],
    cancelled: false,
    paragraphs: texts,
  };
}
