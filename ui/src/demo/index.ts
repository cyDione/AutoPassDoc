import type { Backend } from "../api";
import type {
  AuthorView,
  BlockView,
  Case,
  CommentView,
  FixProgress,
  FixProposal,
  FixSelection,
  FixStage,
  KbDocument,
  KbHit,
  KbImportReport,
  KbProgress,
  ModelProfileView,
  ModelView,
  OpenedDoc,
  PreReviewItem,
  ProbeResult,
  Reviewer,
  ReviewerProfile,
  SpanView,
  Summary,
} from "../types";
import { diffChars } from "./diff";
import { DemoDocument, paragraphText, rewriteSpans, type DemoSource } from "./document";
import { draftFix, seededRandom } from "./fixes";
import {
  GATEWAY_ID,
  KB_PASSAGES,
  gatewayModels,
  seedAliases,
  seedCases,
  seedKbDocuments,
  seedProfiles,
  seedProviders,
  seedReviewers,
  seedSettings,
} from "./seed";

const SAMPLE_PATH = "示例/某市数字政府项目可研报告.docx";
const GENERIC_AUTHOR = /^(administrator|admin|user|author|owner|作者|用户|未知|windows 用户|microsoft office 用户)$/i;
const KB_FORMATS = new Set(["docx", "pdf", "txt", "md"]);

/** Times cross the contract as Unix seconds. */
const unixNow = () => Math.floor(Date.now() / 1000);
const delay = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
const clone = <T,>(value: T): T => structuredClone(value);
const baseName = (path: string) => path.split(/[\\/]/).pop() ?? path;
const extension = (path: string) => /\.([^.\\/]+)$/.exec(path)?.[1].toLowerCase() ?? "";

function levelsFor(reasoning: ModelProfileView["reasoning"]): ModelProfileView["levels"] {
  switch (reasoning) {
    case "effort":
      return ["low", "medium", "high"];
    case "toggle":
      return ["off", "high"];
    case "budget":
      return ["off", "low", "medium", "high"];
    default:
      return [];
  }
}

function guessRole(modelId: string): ModelView["roleHint"] {
  if (/rerank/i.test(modelId)) return "rerank";
  if (/embed|bge-m3|bge-large|e5-/i.test(modelId)) return "embedding";
  if (/jev|laya/i.test(modelId)) return "decision";
  return "chat";
}

/**
 * Browser-only stand-in for the Rust side: serves the sample report exported by
 * `pnpm demo-data` and fakes editing, AI fixes, reviewers, models and the
 * knowledge base in memory, with realistic delays.
 */
export function createDemoBackend(): Backend {
  let source: Promise<DemoSource> | null = null;
  const loadSource = () =>
    (source ??= Promise.all([
      fetch("/demo/summary.json").then((r) => r.json() as Promise<Summary>),
      fetch("/demo/blocks.json").then((r) => r.json() as Promise<BlockView[]>),
    ]).then(([summary, blocks]) => ({ summary, blocks })));

  const docs = new Map<number, DemoDocument>();
  let nextDocId = 1;
  const getDoc = (docId: number) => {
    const doc = docs.get(docId);
    if (!doc) throw new Error("文档已关闭");
    return doc;
  };

  let settings = seedSettings();
  let providers = seedProviders();
  const models = new Map<string, ModelView[]>([[GATEWAY_ID, gatewayModels()]]);
  /** Profiles as detected, before manual overrides. */
  const detected = new Map<string, ModelProfileView>();
  let reviewers = seedReviewers();
  let nextReviewerId = reviewers.length + 1;
  const aliases = seedAliases();
  /** Mappings of generic signatures such as "Administrator" apply to one document only. */
  const docAliases = new Map<number, Map<string, number>>();
  let cases = seedCases();
  let nextCaseId = cases.length + 1;
  const profiles = seedProfiles();
  let kbDocs = seedKbDocuments();
  let nextKbId = kbDocs.length + 1;
  let embedded = kbDocs.reduce((n, d) => n + d.chunkCount, 0) - 236;
  let embedding = false;
  const proposals = new Map<string, FixProposal>();
  const attempts = new Map<string, number>();
  const fixListeners = new Set<(p: FixProgress) => void>();
  const kbListeners = new Set<(p: KbProgress) => void>();

  const withCounts = (r: Reviewer): Reviewer => ({
    ...r,
    caseCount: cases.filter((c) => c.reviewerId === r.id).length,
    profileVersion: profiles.get(r.id)?.version ?? null,
  });
  const findReviewer = (id: number) => {
    const r = reviewers.find((x) => x.id === id);
    if (!r) throw new Error("审稿人不存在");
    return r;
  };
  const aliasTable = (docId: number, author: string) => {
    if (!GENERIC_AUTHOR.test(author)) return aliases;
    let table = docAliases.get(docId);
    if (!table) docAliases.set(docId, (table = new Map()));
    return table;
  };
  const reviewerFor = (docId: number, author: string): Reviewer | null => {
    const id = aliasTable(docId, author).get(author);
    return id === undefined ? null : (reviewers.find((r) => r.id === id) ?? null);
  };
  const addReviewer = (name: string, note: string): Reviewer => {
    const trimmed = name.trim();
    if (!trimmed) throw new Error("请填写审稿人姓名");
    if (reviewers.some((r) => r.name === trimmed)) throw new Error(`已有名为“${trimmed}”的审稿人`);
    const reviewer: Reviewer = { id: nextReviewerId++, name: trimmed, note, threshold: null, caseCount: 0, profileVersion: null };
    reviewers = [...reviewers, reviewer];
    return reviewer;
  };
  const recordCase = (docId: number, proposal: FixProposal, action: Case["action"], finalText: string | null) => {
    const doc = docs.get(docId);
    const comment = doc?.comments.find((c) => c.id === proposal.commentId);
    if (!doc || !comment) return;
    cases = [
      ...cases,
      {
        id: nextCaseId++,
        reviewerId: reviewerFor(docId, comment.author)?.id ?? null,
        docName: doc.fileName,
        commentId: comment.id,
        author: comment.author,
        comment: comment.text,
        original: proposal.paragraphs.map((p) => p.old).join("\n"),
        suggestion: proposal.paragraphs.map((p) => p.new).join("\n"),
        finalText,
        action,
        confidence: proposal.judge?.confidence ?? null,
        category: proposal.judge?.category ?? null,
        createdAt: unixNow(),
      },
    ];
  };

  const authorsOf = (docId: number): AuthorView[] => {
    const byName = new Map<string, AuthorView>();
    for (const c of getDoc(docId).comments) {
      const known = byName.get(c.author);
      if (known) known.commentCount++;
      else
        byName.set(c.author, {
          author: c.author,
          initials: c.initials ?? [...c.author][0] ?? "",
          commentCount: 1,
          reviewer: null,
          generic: GENERIC_AUTHOR.test(c.author),
        });
    }
    return [...byName.values()]
      .map((a) => {
        const r = reviewerFor(docId, a.author);
        return { ...a, reviewer: r && withCounts(r) };
      })
      .sort((a, b) => b.commentCount - a.commentCount);
  };

  const emitFix = (p: FixProgress) => fixListeners.forEach((h) => h(p));
  const emitKb = (p: KbProgress) => kbListeners.forEach((h) => h(p));

  async function runFix(docId: number, commentId: string, selection?: FixSelection | null): Promise<FixProposal> {
    const stage = (s: FixStage) => emitFix({ docId, commentId, stage: s });
    const started = performance.now();
    try {
      const doc = getDoc(docId);
      const comment = doc.comments.find((c) => c.id === commentId);
      if (!comment) throw new Error("批注不存在");
      if (!settings.roles.chat.model) throw new Error("尚未配置大语言模型，请先在设置中选择模型");
      if (comment.paragraphIndex === null) throw new Error("该批注没有锚定到正文段落");
      stage("context");
      await delay(150);
      if (settings.fix.useKb) {
        stage("retrieve");
        await delay(400);
      }
      stage("generate");
      await delay(1200);
      const first = selection ? Math.min(selection.startParagraph, selection.endParagraph) : comment.paragraphIndex;
      const last = selection ? Math.max(selection.startParagraph, selection.endParagraph) : comment.paragraphIndex;
      if (last - first + 1 > 12) throw new Error(`选中了 ${last - first + 1} 段，一次最多修改 12 段，请缩小选区`);
      const extra = [];
      for (let i = first + 1; i <= last; i++) {
        const p = doc.paragraph(i);
        if (p) extra.push(p);
      }
      const paragraph = doc.paragraph(first);
      if (!paragraph) throw new Error("找不到批注所在的段落");
      const original = paragraphText(paragraph);
      const attempt = attempts.get(`${docId}:${commentId}`) ?? 0;
      attempts.set(`${docId}:${commentId}`, attempt + 1);
      const reviewer = reviewerFor(docId, comment.author);
      const draft = draftFix(comment, original, attempt, reviewer?.threshold ?? settings.fix.threshold, settings, kbDocs);
      stage("judge");
      await delay(500);
      const proposal: FixProposal = {
        id: `fix-${docId}-${commentId}-${attempt}`,
        docId,
        commentId,
        reviewer: reviewer?.name ?? null,
        paragraphs: [
          { index: paragraph.index, old: original, new: draft.text, diff: diffChars(original, draft.text) },
          // Extra selected paragraphs: only the wording fix.
          ...extra.map((p) => {
            const old = paragraphText(p);
            const text = old.replace(/进一步/g, "持续");
            return { index: p.index, old, new: text, diff: diffChars(old, text) };
          }),
        ],
        explanation: draft.explanation,
        citations: draft.citations,
        judge: draft.judge,
        judgeError: draft.judgeError,
        model: settings.roles.chat.model,
        elapsedMs: Math.round(performance.now() - started),
        warnings: draft.warnings,
        context: { passages: draft.citations.length * 3, examples: reviewer ? 3 : 0, profile: reviewer !== null && profiles.has(reviewer.id) },
      };
      proposals.set(proposal.id, proposal);
      emitFix({ docId, commentId, stage: "done", proposal });
      return proposal;
    } catch (e) {
      const error = e instanceof Error ? e.message : String(e);
      emitFix({ docId, commentId, stage: "error", error });
      throw new Error(error);
    }
  }

  /** A likely comment on a paragraph, preferring kinds not used yet in this pre-review. */
  function preReviewComment(text: string, used: Set<string>): { comment: string; category: string; at: number } {
    const rules: [RegExp, string, string][] = [
      [/\d+(\.\d+)?万元/, "请说明该金额的测算依据，并与投资估算附表保持一致。", "数据口径"],
      [/《[^》]+》/, "请核实所引文件是否为现行有效版本，并补充发文文号。", "政策依据"],
      [/显著|大幅|明显/, "“显著”等表述缺乏量化支撑，建议给出具体指标。", "措辞规范"],
      [/\d+(\.\d+)?%/, "该比例数据请注明来源和统计口径。", "数据口径"],
      [/风险|措施/, "措施较为笼统，建议明确责任单位和完成时限。", "逻辑结构"],
    ];
    const matches = rules.flatMap(([re, comment, category]) => {
      const m = re.exec(text);
      return m ? [{ comment, category, at: m.index }] : [];
    });
    const pick = matches.find((m) => !used.has(m.comment)) ?? { comment: "本段与前文存在重复表述，建议精简合并。", category: "逻辑结构", at: 0 };
    used.add(pick.comment);
    return pick;
  }

  async function importFiles(paths: string[]): Promise<KbImportReport[]> {
    const reports: KbImportReport[] = [];
    const total = paths.length;
    for (let i = 0; i < total; i++) {
      const fileName = baseName(paths[i]);
      emitKb({ stage: "import", done: i, total, current: fileName });
      await delay(450);
      const format = extension(fileName);
      const existing = kbDocs.find((d) => d.fileName === fileName);
      if (!KB_FORMATS.has(format)) {
        reports.push({ fileName, docId: null, chunks: 0, unchanged: false, warnings: [], error: "不支持的文件格式，仅支持 Word、PDF、TXT 和 Markdown" });
      } else if (existing) {
        reports.push({ fileName, docId: existing.id, chunks: existing.chunkCount, unchanged: true, warnings: [], error: null });
      } else {
        const random = seededRandom(fileName);
        const chunkCount = 20 + Math.floor(random() * 180);
        const title = fileName.replace(/\.[^.]+$/, "");
        const numbered = /〔\d{4}〕\d+号/.exec(title)?.[0] ?? null;
        const warnings = numbered ? [] : ["未识别到文号、发文机关和日期，可在列表中手动补充。"];
        if (format === "pdf" && random() < 0.5) warnings.unshift("第 12–14 页是扫描图片，未能提取文字（需要 OCR）。");
        const doc: KbDocument & { warnings: string[] } = {
          id: nextKbId++,
          title,
          fileName,
          storedPath: `C:\\Users\\演示\\AppData\\Roaming\\AutoPassDoc\\kb\\${fileName}`,
          originalPath: paths[i],
          format,
          meta: { title, docNumber: numbered, issuer: null, date: null },
          chunkCount,
          charCount: chunkCount * 260,
          importedAt: unixNow(),
          sha256: fileName,
          warnings,
        };
        kbDocs = [doc, ...kbDocs];
        reports.push({ fileName, docId: doc.id, chunks: chunkCount, unchanged: false, warnings, error: null });
      }
    }
    emitKb({ stage: "import", done: total, total });
    // The Rust side embeds new documents right after an import when an embedding model is set.
    if (settings.roles.embedding.model && reports.some((r) => r.docId !== null && !r.unchanged)) void embedPending();
    return reports;
  }

  const totalChunks = () => kbDocs.reduce((n, d) => n + d.chunkCount, 0);

  /**
   * Embeds pending chunks in the background. Like the Rust side, progress
   * counts over the whole knowledge base, and a failure arrives as an event
   * with `total` 0 and a message.
   */
  async function embedPending() {
    if (embedding) return;
    embedding = true;
    try {
      const provider = providers.find((p) => p.id === settings.roles.embedding.providerId);
      for (;;) {
        const total = totalChunks();
        emitKb({ stage: "embed", done: Math.min(embedded, total), total });
        if (embedded >= total) return;
        await delay(140);
        if (!provider || (!provider.hasKey && provider.kind !== "ollama")) throw new Error("向量模型调用失败：401 未授权，请检查 API Key");
        embedded = Math.min(total, embedded + 32);
      }
    } catch (e) {
      emitKb({ stage: "embed", done: 0, total: 0, message: e instanceof Error ? e.message : String(e) });
    } finally {
      embedding = false;
    }
  }

  const open = async (path: string): Promise<OpenedDoc> => {
    const doc = new DemoDocument(nextDocId++, path, await loadSource());
    docs.set(doc.docId, doc);
    return { docId: doc.docId, path: doc.path, fileName: doc.fileName, summary: doc.summary };
  };

  return {
    open,
    pickAndOpen: () => open(SAMPLE_PATH),
    blocks: async (docId, start, end) => {
      await loadSource();
      return getDoc(docId).blocks(start, end);
    },
    async saveAs(docId) {
      const doc = getDoc(docId);
      await delay(250);
      const path = doc.path.replace(/(\.docx)?$/i, "_AutoPassDoc.docx");
      doc.markSaved(path);
      return path;
    },
    async close(docId) {
      docs.delete(docId);
    },
    async initialFile() {
      return null;
    },
    imageUrl: (_docId, relId) => `/demo/${relId}.png`,
    onFileDrop(handler, onHover) {
      // Browsers hide file paths; the file names stand in for them.
      let depth = 0;
      const hasFiles = (e: DragEvent) => e.dataTransfer?.types.includes("Files") ?? false;
      const enter = (e: DragEvent) => {
        if (!hasFiles(e)) return;
        e.preventDefault();
        depth++;
        onHover(true);
      };
      const over = (e: DragEvent) => {
        if (hasFiles(e)) e.preventDefault();
      };
      const leave = () => {
        depth = Math.max(0, depth - 1);
        if (depth === 0) onHover(false);
      };
      const drop = (e: DragEvent) => {
        e.preventDefault();
        depth = 0;
        onHover(false);
        const names = [...(e.dataTransfer?.files ?? [])].map((f) => f.name);
        if (names.length) handler(names);
      };
      window.addEventListener("dragenter", enter);
      window.addEventListener("dragover", over);
      window.addEventListener("dragleave", leave);
      window.addEventListener("drop", drop);
      return () => {
        window.removeEventListener("dragenter", enter);
        window.removeEventListener("dragover", over);
        window.removeEventListener("dragleave", leave);
        window.removeEventListener("drop", drop);
      };
    },

    // ---- Editing ----
    docState: async (docId) => getDoc(docId).state,
    summary: async (docId) => getDoc(docId).summary,
    undo: async (docId) => getDoc(docId).undo(),
    redo: async (docId) => getDoc(docId).redo(),
    async save(docId) {
      const doc = getDoc(docId);
      if (!doc.savedPath) throw new Error("文档还没有保存过，请先另存为");
      await delay(200);
      doc.markSaved(doc.savedPath);
      return doc.state;
    },
    async setCommentDone(docId, commentId, done) {
      const doc = getDoc(docId);
      const comments = doc.comments.map((c) => (c.id === commentId ? { ...c, done } : c));
      return doc.commit(done ? "标记批注已解决" : "重新打开批注", { comments });
    },

    // ---- AI fixes ----
    fixComment: runFix,
    async fixBatch(docId, commentIds) {
      getDoc(docId);
      const queue = [...commentIds];
      const worker = async () => {
        for (let id = queue.shift(); id !== undefined; id = queue.shift()) await runFix(docId, id).catch(() => {});
      };
      for (let i = 0; i < Math.max(1, settings.fix.concurrency); i++) void worker();
    },
    onFixProgress(handler) {
      fixListeners.add(handler);
      return () => fixListeners.delete(handler);
    },
    async applyFix(docId, proposalId, edited, force) {
      const proposal = proposals.get(proposalId);
      if (!proposal || proposal.docId !== docId) throw new Error("修改建议已失效，请重新生成");
      if (!edited && !force && proposal.judge && !proposal.judge.passed) throw new Error("置信度未达标，需要确认后才能应用");
      await delay(120);
      const doc = getDoc(docId);
      const tracked = settings.fix.editMode === "tracked";
      const paragraphs = new Map<number, SpanView[]>();
      proposal.paragraphs.forEach((fp, i) => {
        const p = doc.paragraph(fp.index);
        if (!p) throw new Error("找不到要修改的段落");
        const current = paragraphText(p);
        if (current !== fp.old) throw new Error("段落在生成建议后已被修改，请重新生成");
        paragraphs.set(fp.index, rewriteSpans(p.spans, diffChars(current, edited?.[i] ?? fp.new), tracked, settings.fix.author));
      });
      const target = doc.comments.find((c) => c.id === proposal.commentId);
      let comments: CommentView[] = doc.comments.map((c) =>
        c.id === proposal.commentId && settings.fix.resolveOnApply ? { ...c, done: true } : c,
      );
      if (target && settings.fix.replyOnApply && settings.fix.replyText.trim()) {
        comments = [
          ...comments,
          {
            id: `reply-${proposal.id}`,
            author: settings.fix.author,
            initials: [...settings.fix.author][0] ?? "A",
            date: new Date().toISOString(),
            text: settings.fix.replyText,
            parentId: target.id,
            done: false,
            blockIndex: target.blockIndex,
            paragraphIndex: target.paragraphIndex,
            quote: "",
          },
        ];
      }
      const outcome = doc.commit("AI 修复", { paragraphs, comments });
      const finalText = (edited ?? proposal.paragraphs.map((p) => p.new)).join("\n");
      const changed = edited !== null && edited.some((text, i) => text !== proposal.paragraphs[i]?.new);
      recordCase(docId, proposal, changed ? "edited" : "accepted", finalText);
      proposals.delete(proposalId);
      return outcome;
    },
    async rejectFix(proposalId) {
      const proposal = proposals.get(proposalId);
      if (!proposal) return;
      recordCase(proposal.docId, proposal, "rejected", null);
      proposals.delete(proposalId);
    },

    // ---- Reviewers ----
    documentAuthors: async (docId) => authorsOf(docId),
    async assignAuthor(docId, author, _initials, reviewerId, newReviewer, writeBack) {
      await delay(200);
      const target = newReviewer ? addReviewer(newReviewer, "") : reviewerId !== null ? findReviewer(reviewerId) : null;
      const table = aliasTable(docId, author);
      if (target) table.set(author, target.id);
      else table.delete(author);
      let outcome = null;
      if (writeBack && target && target.name !== author) {
        const doc = getDoc(docId);
        const comments = doc.comments.map((c) => (c.author === author ? { ...c, author: target.name, initials: [...target.name][0] } : c));
        aliases.set(target.name, target.id);
        outcome = doc.commit("修改批注署名", { comments });
      }
      return { authors: authorsOf(docId), outcome };
    },
    reviewers: async () => reviewers.map(withCounts),
    createReviewer: async (name, note) => withCounts(addReviewer(name, note)),
    async updateReviewer(id, name, note, threshold) {
      const trimmed = name.trim();
      if (!trimmed) throw new Error("请填写审稿人姓名");
      if (reviewers.some((r) => r.id !== id && r.name === trimmed)) throw new Error(`已有名为“${trimmed}”的审稿人`);
      const updated = { ...findReviewer(id), name: trimmed, note, threshold };
      reviewers = reviewers.map((r) => (r.id === id ? updated : r));
      return withCounts(updated);
    },
    async deleteReviewer(id) {
      reviewers = reviewers.filter((r) => r.id !== id);
      for (const table of [aliases, ...docAliases.values()]) for (const [k, v] of table) if (v === id) table.delete(k);
      cases = cases.map((c) => (c.reviewerId === id ? { ...c, reviewerId: null } : c));
      profiles.delete(id);
    },
    async mergeReviewers(from, into) {
      if (from === into) throw new Error("不能合并到自己");
      findReviewer(into);
      for (const table of [aliases, ...docAliases.values()]) for (const [k, v] of table) if (v === from) table.set(k, into);
      cases = cases.map((c) => (c.reviewerId === from ? { ...c, reviewerId: into } : c));
      reviewers = reviewers.filter((r) => r.id !== from);
      profiles.delete(from);
    },
    reviewerProfile: async (id) => clone(profiles.get(id) ?? null),
    async distillProfile(id) {
      findReviewer(id);
      await delay(1500);
      const decided = cases.filter((c) => c.reviewerId === id && c.action !== "pending");
      if (decided.length < 3) throw new Error(`已处理的案例只有 ${decided.length} 条，至少需要 3 条才能提炼画像。`);
      const byCategory = new Map<string, number>();
      for (const c of decided) if (c.category) byCategory.set(c.category, (byCategory.get(c.category) ?? 0) + 1);
      const top = [...byCategory.entries()].sort((a, b) => b[1] - a[1]).map(([k]) => k);
      const previous = profiles.get(id);
      const profile: ReviewerProfile = {
        reviewerId: id,
        version: (previous?.version ?? 0) + 1,
        createdAt: unixNow(),
        caseCount: decided.length,
        summary: `该审稿人最常提出${top.slice(0, 2).join("和") || "表述"}方面的意见，倾向于小范围、有出处的修改；被拒绝的建议多为改动过大或引用依据不充分。`,
        focus: top.map((k) => `${k}（${byCategory.get(k)} 条）`),
        preferences: ["修改尽量局部，保留原有段落结构", "数据和引用后用括号注明出处"],
        commonRequests: [...new Set(decided.map((c) => c.comment))].slice(0, 3),
      };
      profiles.set(id, profile);
      return clone(profile);
    },
    reviewerCases: async (id) => cases.filter((c) => c.reviewerId === id).sort((a, b) => b.createdAt - a.createdAt),
    async preReview(docId, reviewerId, start, end) {
      const reviewer = findReviewer(reviewerId);
      await delay(1400);
      if (!profiles.has(reviewerId) && !cases.some((c) => c.reviewerId === reviewerId)) {
        throw new Error(`还不了解「${reviewer.name}」的审稿习惯：先用 AI 修复处理几条他的批注，或在审稿人页面提炼画像`);
      }
      const candidates = getDoc(docId)
        .paragraphsIn(start, end)
        .filter((p) => p.headingLevel === null)
        .map((p) => ({ index: p.index, text: paragraphText(p) }))
        .filter((p) => p.text.length >= 40);
      const count = Math.min(candidates.length, 3 + (reviewerId % 2));
      const items: PreReviewItem[] = [];
      const used = new Set<string>();
      for (let i = 0; i < count; i++) {
        const p = candidates[Math.floor((i * candidates.length) / count)];
        const { comment, category, at } = preReviewComment(p.text, used);
        const from = Math.max(0, at - 12);
        items.push({ paragraphIndex: p.index, quote: p.text.slice(from, from + 42), comment, category });
      }
      return items;
    },
    async exportDataset() {
      await delay(500);
      const path = "D:\\导出\\autopassdoc-judge-dataset.jsonl";
      console.info(`[演示] 已导出 ${cases.filter((c) => c.action !== "pending").length} 条案例到 ${path}`);
      return path;
    },

    // ---- Settings and models ----
    settings: async () => clone(settings),
    async saveSettings(next) {
      settings = clone(next);
      return clone(settings);
    },
    providers: async () => clone(providers),
    async saveProvider(provider, apiKey) {
      if (!provider.name.trim()) throw new Error("请填写服务商名称");
      if (!provider.baseUrl.trim()) throw new Error("请填写服务地址");
      const existing = providers.find((p) => p.id === provider.id);
      const hasKey = apiKey === undefined ? (existing?.hasKey ?? false) : !!apiKey;
      const saved = { ...provider, hasKey };
      providers = existing ? providers.map((p) => (p.id === provider.id ? saved : p)) : [...providers, saved];
      return clone(saved);
    },
    async deleteProvider(id) {
      providers = providers.filter((p) => p.id !== id);
      models.delete(id);
    },
    async fetchModels(providerId) {
      const provider = providers.find((p) => p.id === providerId);
      if (!provider) throw new Error("服务商不存在，请先保存");
      await delay(700);
      if (!provider.hasKey && provider.kind !== "ollama") throw new Error("401 未授权：请检查 API Key");
      const previous = models.get(providerId) ?? [];
      const list = gatewayModels().map((m) => previous.find((p) => p.id === m.id && p.manual) ?? m);
      models.set(providerId, list);
      return clone(list);
    },
    providerModels: async (providerId) => clone(models.get(providerId) ?? []),
    async setModelProfile(providerId, modelId, override) {
      const list = models.get(providerId) ?? [];
      models.set(providerId, list);
      let model = list.find((m) => m.id === modelId);
      if (!model) {
        model = {
          id: modelId,
          roleHint: guessRole(modelId),
          profile: { contextWindow: 32_768, maxOutputTokens: 4_096, levels: [], reasoning: "none", jsonMode: false, source: "default" },
          manual: false,
        };
        list.push(model);
      }
      const key = `${providerId}\n${modelId}`;
      if (!detected.has(key)) detected.set(key, model.profile);
      const base = detected.get(key)!;
      const updated: ModelView = override
        ? {
            ...model,
            manual: true,
            profile: {
              contextWindow: override.contextWindow ?? base.contextWindow,
              maxOutputTokens: override.maxOutputTokens ?? base.maxOutputTokens,
              reasoning: override.reasoning ?? base.reasoning,
              levels: override.reasoning ? levelsFor(override.reasoning) : base.levels,
              jsonMode: override.jsonMode ?? base.jsonMode,
              source: "manual",
            },
          }
        : { ...model, manual: false, profile: base };
      list[list.indexOf(model)] = updated;
      return clone(updated);
    },
    async testRole(role): Promise<ProbeResult> {
      await delay(600);
      const config = settings.roles[role];
      if (!config.model) return { ok: false, latencyMs: 0, summary: "尚未选择模型" };
      const provider = providers.find((p) => p.id === config.providerId);
      if (!provider) return { ok: false, latencyMs: 0, summary: "服务商不存在或已删除" };
      const summaries = {
        chat: `回复正常：“你好，我是 ${config.model}，可以协助按批注修改报告。”`,
        decision: "决策接口正常：测试题“该句是否为陈述句？”P(是) = 0.97",
        embedding: "向量接口正常：维度 1024",
        rerank: "重排接口正常：3 个候选，最高分 0.93",
      };
      return { ok: true, latencyMs: 280 + Math.round(Math.random() * 600), summary: summaries[role] };
    },

    // ---- Knowledge base ----
    kbDocuments: async () => clone(kbDocs),
    async kbStats() {
      const chunks = totalChunks();
      return { documents: kbDocs.length, chunks, embedded: Math.min(embedded, chunks), embeddingModel: settings.roles.embedding.model || null };
    },
    kbImport: (paths) => importFiles(paths ?? ["数据安全管理办法（2024年修订）.pdf", "某市政务云管理暂行办法.docx"]),
    async kbRemove(docId) {
      const doc = kbDocs.find((d) => d.id === docId);
      kbDocs = kbDocs.filter((d) => d.id !== docId);
      if (doc) embedded = Math.max(0, embedded - doc.chunkCount);
    },
    async kbUpdateMeta(docId, meta) {
      const doc = kbDocs.find((d) => d.id === docId);
      if (!doc) throw new Error("资料不存在");
      const updated = { ...doc, meta: clone(meta), title: meta.title?.trim() || doc.title };
      kbDocs = kbDocs.map((d) => (d.id === docId ? updated : d));
      return clone(updated);
    },
    async kbSearch(text) {
      const query = text.trim();
      if (!query) return [];
      await delay(250);
      const rerank = !!settings.roles.rerank.model;
      return KB_PASSAGES.flatMap((p, i): KbHit[] => {
        const doc = kbDocs.find((d) => d.id === p.docId);
        if (!doc) return [];
        const cut = p.text.indexOf("，") + 1;
        const passage = `${p.text.slice(0, cut)}涉及${query}的，${p.text.slice(cut)}`;
        return [
          {
            chunkId: p.docId * 1000 + i,
            docId: doc.id,
            fileName: doc.fileName,
            title: doc.title,
            storedPath: doc.storedPath,
            headingPath: p.headingPath,
            text: passage,
            parentText: passage,
            charStart: i * 400,
            charEnd: i * 400 + passage.length,
            keywordRank: i % 3 === 2 ? null : i + 1,
            vectorRank: i % 4 === 1 ? null : ((i * 2) % KB_PASSAGES.length) + 1,
            score: 0.032 - i * 0.003,
            rerankScore: rerank ? 0.94 - i * 0.11 : null,
          },
        ];
      });
    },
    async kbEmbed() {
      if (!settings.roles.embedding.model) throw new Error("尚未配置向量模型");
      if (!providers.some((p) => p.id === settings.roles.embedding.providerId)) throw new Error("向量模型的服务商不存在或已删除");
      void embedPending();
    },
    async kbClearEmbeddings() {
      embedded = 0;
    },
    onKbProgress(handler) {
      kbListeners.add(handler);
      return () => kbListeners.delete(handler);
    },
    async openPath(path) {
      console.info(`[演示] 用系统默认程序打开：${path}`);
    },

    confirm: async (message) => window.confirm(message),
    onCloseRequested: () => () => {},
  };
}
