import type { Citation, CommentView, FixMode, JudgeItem, Judgement, KbDocument, Settings } from "../types";
import { KB_PASSAGES } from "./seed";

/** Deterministic pseudo-random numbers, so a comment's first attempt always looks the same. */
export function seededRandom(seed: string): () => number {
  let h = 2166136261;
  for (const ch of seed) h = Math.imul(h ^ ch.charCodeAt(0), 16777619);
  return () => {
    h = (h + 0x6d2b79f5) | 0;
    let t = Math.imul(h ^ (h >>> 15), 1 | h);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const PHRASES: [string, string][] = [
  ["围绕", "聚焦"],
  ["进一步", "持续"],
  ["有序推进", "稳步推进"],
  ["突出问题", "短板弱项"],
  ["相应的", "针对性的"],
  ["开展系统研究", "开展专题研究"],
  ["确保", "切实保障"],
  ["建设必要性充分", "建设十分必要"],
];

function insertBeforePeriod(text: string, insert: string): string {
  const at = text.indexOf("。");
  return at < 0 ? text + insert : text.slice(0, at) + insert + text.slice(at);
}

function replacePhrase(text: string, pick: number): { text: string; note: string } | null {
  for (let i = 0; i < PHRASES.length; i++) {
    const [from, to] = PHRASES[(pick + i) % PHRASES.length];
    if (text.includes(from)) return { text: text.replace(from, to), note: `将“${from}”调整为“${to}”` };
  }
  return null;
}

interface Rewrite {
  text: string;
  notes: string[];
  category: string;
  warnings: string[];
}

/** A plausible rewrite of the paragraph for the comment's request. */
function rewrite(comment: string, original: string, pick: number): Rewrite {
  const notes: string[] = [];
  const warnings: string[] = [];
  let text = original;
  let category = "其他";
  let withPhrase = true;
  const insert = (what: string, note: string) => {
    text = insertBeforePeriod(text, what);
    notes.push(note);
  };

  if (/标准已废止/.test(comment)) {
    category = "政策依据";
    const std = /GB\/T\s?\d+-(\d{4})/.exec(text);
    if (std) {
      text = text.replace(std[0], std[0].replace(std[1], "2024"));
      notes.push("将引用标准更新为现行版本");
    } else insert("（依据现行有效版本）", "注明引用依据为现行有效版本");
  } else if (/政策依据|文号/.test(comment)) {
    category = "政策依据";
    insert("（依据《政务信息化项目建设管理办法》（国办发〔2024〕12号））", "补充政策依据和文号");
  } else if (/口语化/.test(comment)) {
    category = "措辞规范";
    const first = replacePhrase(text, pick);
    if (first) {
      text = first.text;
      notes.push(first.note);
    }
    pick += 3;
  } else if (/对标/.test(comment)) {
    const at = text.indexOf("。");
    const sentence = "经与杭州、深圳等国内先进地区对标，本市在政务数据共享率、“一网通办”覆盖率等方面仍有一定差距。";
    text = at < 0 ? text + "。" + sentence : text.slice(0, at + 1) + sentence + text.slice(at + 1);
    notes.push("补充与国内先进地区的对标分析");
  } else if (/统计年鉴|数据口径/.test(comment)) {
    category = "数据口径";
    // The first attempt lacks the figure, as a real model would without a source.
    if (pick % 2 === 0) insert("（数据来源：【待补充：2024年市统计年鉴中的对应数据】）", "标出需要补充的数据来源");
    else insert("（数据来源：2024年市统计年鉴）", "补充数据来源");
  } else if (/测算方法/.test(comment)) {
    category = "数据口径";
    insert("（按工程量清单法测算，主要单价参照2024年本市信息化项目预算编制标准）", "说明测算方法和参数依据");
  } else if (/投资估算/.test(comment)) {
    category = "数据口径";
    const amount = /(\d+)万元/.exec(text);
    if (amount) {
      text = text.replace(amount[0], `${Number(amount[1]) + 12}万元（与附表3-2一致）`);
      notes.push("按附表统一投资金额");
      warnings.push("修改涉及金额，请与附表逐项核对。");
    } else insert("（与附表3-2一致）", "注明与附表一致");
  } else if (/显著/.test(comment)) {
    category = "措辞规范";
    if (text.includes("显著的")) text = text.replace("显著的", "较为明显的");
    insert("（预计业务办理时长缩短30%以上）", "用量化指标替代“显著”");
  } else if (/格式问题/.test(comment)) {
    category = "格式";
    const spaced = /(\d)\s+(?=[万亿个项%元年月日])/g;
    if (spaced.test(text)) {
      text = text.replace(spaced, "$1");
      notes.push("删除数字与单位之间的空格");
      withPhrase = false;
    }
  } else if (/风险应对|责任单位/.test(comment)) {
    category = "逻辑结构";
    insert("，由市大数据局牵头、各相关部门配合，于2025年6月底前完成", "细化责任单位和时间节点");
  } else if (/重复内容/.test(comment)) {
    category = "逻辑结构";
    const first = text.indexOf("。");
    const second = first < 0 ? -1 : text.indexOf("。", first + 1);
    if (second > 0 && second < text.length - 1) {
      text = text.slice(0, first + 1) + text.slice(second + 1);
      notes.push("删除与第二章重复的表述");
      withPhrase = false;
    }
  } else if (/逻辑不够清晰/.test(comment)) {
    category = "逻辑结构";
    text = "针对现有信息系统分散建设、数据共享不足等问题，" + text;
    notes.push("调整为先讲问题、再讲措施");
  } else {
    insert("（数据来源：2024年市统计年鉴）", "补充数据来源");
  }

  if (withPhrase) {
    const phrase = replacePhrase(text, pick);
    if (phrase) {
      text = phrase.text;
      notes.push(phrase.note);
    }
  }
  if (text === original) {
    text = insertBeforePeriod(text, "（已按意见核实）");
    notes.push("注明已核实");
  }
  return { text, notes, category, warnings };
}

const JUDGE_ITEMS: Omit<JudgeItem, "value" | "passed">[] = [
  { key: "addresses", label: "回应批注", weight: 0.4, hard: true },
  { key: "meaning", label: "保持原意", weight: 0.25, hard: true },
  { key: "grounded", label: "有据可查", weight: 0.25, hard: true },
  { key: "style", label: "公文规范", weight: 0.1, hard: false },
];

/** Score-type items pass at an expected 3.5 of 5. */
const SOFT_PASS = 0.625;

function judge(random: () => number, threshold: number, settings: Settings, category: string): Judgement {
  const between = (lo: number, hi: number) => lo + random() * (hi - lo);
  const outcome = random();
  const values = JUDGE_ITEMS.map(() => between(0.84, 0.98));
  if (outcome > 0.85) {
    // One hard item clearly fails, even if the average looks fine.
    values[1 + Math.floor(random() * 2)] = between(0.42, 0.72);
  } else if (outcome > 0.65) {
    for (let i = 0; i < values.length; i++) values[i] = between(0.62, 0.8);
  }
  const items: JudgeItem[] = JUDGE_ITEMS.map((item, i) => ({
    ...item,
    value: values[i],
    passed: values[i] >= (item.hard ? threshold : SOFT_PASS),
  }));
  const confidence = items.reduce((sum, item) => sum + item.value * item.weight, 0) / items.reduce((sum, item) => sum + item.weight, 0);
  const jev = settings.roles.decisionBackend === "jev";
  return {
    confidence,
    threshold,
    passed: confidence >= threshold && items.every((item) => !item.hard || item.passed),
    items,
    category,
    backend: jev ? "jev" : "chat",
    model: (jev ? settings.roles.decision.model : settings.roles.decision.model || settings.roles.chat.model) || "jev-latest",
  };
}

function citations(random: () => number, kb: KbDocument[], count: number): Citation[] {
  const passages = KB_PASSAGES.map((p, i) => ({ ...p, i, doc: kb.find((d) => d.id === p.docId) })).filter((p) => p.doc);
  const picked: typeof passages = [];
  while (picked.length < Math.min(count, passages.length)) {
    const p = passages[Math.floor(random() * passages.length)];
    if (!picked.includes(p)) picked.push(p);
  }
  return picked.map((p, n) => {
    const charStart = Math.floor(random() * 20_000);
    return {
      n: n + 1,
      docId: p.docId,
      chunkId: p.docId * 1000 + p.i,
      fileName: p.doc!.fileName,
      title: p.doc!.title,
      headingPath: p.headingPath,
      text: p.text,
      storedPath: p.doc!.storedPath,
      charStart,
      charEnd: charStart + p.text.length,
    };
  });
}

export interface DraftFix {
  text: string;
  explanation: string;
  judge: Judgement | null;
  judgeError: string | null;
  citations: Citation[];
  warnings: string[];
}

export function draftFix(
  comment: CommentView,
  original: string,
  attempt: number,
  threshold: number,
  settings: Settings,
  kb: KbDocument[],
  mode: FixMode = "fix",
  direction = "",
): DraftFix {
  const random = seededRandom(`${comment.id}:${attempt}`);
  const drafted = rewrite(comment.text, original, attempt + Math.floor(random() * PHRASES.length));
  const { category, warnings } = drafted;
  let { text, notes } = drafted;
  if (mode === "rewrite") {
    // A rewrite restates the whole paragraph instead of patching one phrase.
    text = text
      .replace(/进一步/g, "持续")
      .replace(/显著的?/g, "明显")
      .replace(/^(.{0,40}?)，/, "$1。在此基础上，");
    notes = ["按批注和上下文重写了整段表述", ...notes.slice(0, 1)];
  }
  if (direction) notes = [`按修改方向“${direction.length > 20 ? `${direction.slice(0, 20)}…` : direction}”调整`, ...notes];
  const cites = settings.fix.useKb ? citations(random, kb, 2) : [];
  let judgement: Judgement | null = null;
  let judgeError: string | null = null;
  const decisionModel = settings.roles.decisionBackend === "jev" ? settings.roles.decision.model : settings.roles.chat.model;
  if (!decisionModel) judgeError = "尚未配置决策模型，无法评判置信度";
  else if (random() < 0.1) judgeError = `决策模型请求超时（${decisionModel}，30 秒）`;
  else judgement = judge(random, threshold, settings, category);
  if (cites.length && random() < 0.25) warnings.push(`引用资料《${cites[0].title}》发布时间较早，请确认仍然有效。`);
  const refs = cites.length ? `，参考了${cites.map((c) => `[${c.n}]`).join("")}中的相关规定` : "";
  return {
    text,
    explanation: `根据批注意见，${notes.join("，")}${refs}，其余表述保持不变。`,
    judge: judgement,
    judgeError,
    citations: cites,
    warnings,
  };
}
