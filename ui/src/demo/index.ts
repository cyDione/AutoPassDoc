import type { Backend } from "../api";
import type {
  UpdateProgress,
  AuthorView,
  BlockView,
  Case,
  CommentView,
  FixProgress,
  FixProposal,
  FixRequest,
  FixStage,
  BackupManifest,
  BackupProgress,
  KbDocument,
  KbDocumentView,
  KbHit,
  KbImportReport,
  KbProgress,
  ModelProfileView,
  ModelView,
  OpenedDoc,
  ParserInfo,
  ParserKind,
  PreReviewItem,
  ProofIssue,
  ProofProgress,
  ProbeResult,
  Reviewer,
  ReviewerProfile,
  SpanView,
  Summary,
} from "../types";
import type { SearchService, SearchServiceInfo } from "../types";
import { PLACEHOLDER } from "../types";
import { diffChars } from "./diff";
import { DemoDocument, paragraphText, rewriteSpans, type DemoSource } from "./document";
import { draftFix, seededRandom } from "./fixes";
import { demoProofread } from "./proofread";
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

const SEARCH_SERVICES: Omit<SearchServiceInfo, "hasKey">[] = [
  { kind: "zhipu", name: "智谱搜索", keyUrl: "https://bigmodel.cn/usercenter/proj-mgmt/apikeys", note: "与智谱大模型共用 API Key，基础版约 0.01 元/次" },
  { kind: "bocha", name: "博查搜索", keyUrl: "https://open.bochaai.com", note: "国内网页搜索 API，按次计费" },
  { kind: "tavily", name: "Tavily", keyUrl: "https://app.tavily.com", note: "海外服务，每月有免费额度，国内网站覆盖较少" },
];

const SAMPLE_PATH = "示例/某市数字政府项目可研报告.docx";
const GENERIC_AUTHOR = /^(administrator|admin|user|author|owner|作者|用户|未知|windows 用户|microsoft office 用户)$/i;
const KB_FORMATS = new Set(["docx", "pdf", "txt", "md"]);
const IMAGE_FORMATS = new Set(["png", "jpg", "jpeg", "bmp", "tif", "tiff", "webp"]);
const DEMO_FOLDER = "D:\\资料\\上海市政策文件";
/** What the demo folder holds; the images are only read in enhanced mode. */
const FOLDER_FILES = [
  "上海市公共数据开放实施细则.docx",
  "上海市数据条例.pdf",
  "关于进一步促进上海市数字经济发展的若干意见.docx",
  "上海市政务信息化项目管理办法（2025年修订）.pdf",
  "子目录\\崇明区生态岛建设规划（2021-2035年）.pdf",
  "子目录\\崇明区统计公报扫描件.png",
  "说明.txt",
];
const PARSER_INFO: Record<Exclude<ParserKind, "builtin">, Omit<ParserInfo, "hasKey">> = {
  mineru: {
    kind: "mineru",
    name: "MinerU",
    siteUrl: "https://mineru.net",
    keyUrl: "https://mineru.net/apiManage/token",
    docsUrl: "https://mineru.net/apiManage/docs",
    consoleUrl: "https://mineru.net/apiManage/token",
    quotaNote: "MinerU 暂未提供余额查询接口，每天有免费页数，用量请在官网控制台查看。",
  },
  paddleocr: {
    kind: "paddleocr",
    name: "PaddleOCR",
    siteUrl: "https://aistudio.baidu.com/paddleocr",
    keyUrl: "https://aistudio.baidu.com/account/accessToken",
    docsUrl: "https://ai.baidu.com/ai-doc/AISTUDIO/Kmfl2ycs0",
    consoleUrl: "https://aistudio.baidu.com/account/accessToken",
    quotaNote: "PaddleOCR（AI Studio）暂未提供余额查询接口，用量请在官网查看。",
  },
};

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

  async function runFix(docId: number, commentId: string, request?: FixRequest): Promise<FixProposal> {
    const selection = request?.selection ?? null;
    const mode = request?.mode ?? "fix";
    const direction = request?.direction?.trim() ?? "";
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
      if (request?.sources?.length || (settings.fix.useWeb && /《|规划|政策|标准|十四五|十五五/.test(`${direction}${comment.text}`))) {
        stage("web");
        await delay(700);
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
      const draft = draftFix(comment, original, attempt, reviewer?.threshold ?? settings.fix.threshold, settings, kbDocs, mode, direction);
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
        context: { passages: draft.citations.length * 3, related: 2, examples: reviewer ? 3 : 0, profile: reviewer !== null && profiles.has(reviewer.id) },
        mode,
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

  const parserKeys = new Set<ParserKind>();
  const searchKeys = new Set<SearchService>();
  const enhanced = () => (settings.kb.parser !== "builtin" && parserKeys.has(settings.kb.parser) ? settings.kb.parser : null);
  const expandPaths = (paths: string[]) =>
    paths.flatMap((p) =>
      p === DEMO_FOLDER
        ? FOLDER_FILES.filter((f) => enhanced() || !IMAGE_FORMATS.has(extension(f))).map((f) => `${DEMO_FOLDER}\\${f}`)
        : [p],
    );
  const proofListeners = new Set<(p: ProofProgress) => void>();
  let proofCancel = false;
  const backupListeners = new Set<(p: BackupProgress) => void>();
  const updateListeners = new Set<(p: UpdateProgress) => void>();
  let updateCancelled = false;
  const demoInstaller = { name: "AutoPassDoc_0.3.1_x64-setup.exe", url: "https://github.com/cyDione/AutoPassDoc/releases", size: 9_800_000 };
  const backupManifest = (): BackupManifest => ({
    format: 1,
    appVersion: "0.3.0",
    createdAt: unixNow(),
    documents: kbDocs.length,
    chunks: totalChunks(),
    reviewers: reviewers.length,
    cases: cases.length,
  });
  async function backupSteps(steps: string[]) {
    for (let i = 0; i < steps.length; i++) {
      backupListeners.forEach((h) => h({ done: i, total: steps.length, step: steps[i] }));
      await delay(350);
    }
    backupListeners.forEach((h) => h({ done: steps.length, total: steps.length, step: "完成" }));
  }

  async function importFiles(input: string[]): Promise<KbImportReport[]> {
    const paths = expandPaths(input);
    const reports: KbImportReport[] = [];
    const total = paths.length;
    for (let i = 0; i < total; i++) {
      const fileName = baseName(paths[i]);
      emitKb({ stage: "import", done: i, total, current: fileName });
      await delay(450);
      const format = extension(fileName);
      const existing = kbDocs.find((d) => d.fileName === fileName);
      const parser = enhanced() && (format === "pdf" || IMAGE_FORMATS.has(format)) ? enhanced()! : "builtin";
      if (parser !== "builtin") {
        for (let page = 1; page <= 3; page++) {
          emitKb({ stage: "import", done: i, total, current: `${fileName}（${PARSER_INFO[parser].name} 解析中 ${page}/3 页）` });
          await delay(300);
        }
      }
      if (IMAGE_FORMATS.has(format) && parser === "builtin") {
        reports.push({ fileName, docId: null, chunks: 0, unchanged: false, warnings: [], error: "图片需要增强解析：请在「设置 → 知识库」中选择 MinerU 或 PaddleOCR 并保存 Key 后再导入", parser });
      } else if (!KB_FORMATS.has(format) && !IMAGE_FORMATS.has(format)) {
        reports.push({ fileName, docId: null, chunks: 0, unchanged: false, warnings: [], error: "不支持的文件格式，仅支持 Word、PDF、TXT 和 Markdown" });
      } else if (existing) {
        reports.push({ fileName, docId: existing.id, chunks: existing.chunkCount, unchanged: true, warnings: [], error: null });
      } else {
        const random = seededRandom(fileName);
        const chunkCount = 20 + Math.floor(random() * 180);
        const title = fileName.replace(/\.[^.]+$/, "");
        const numbered = /〔\d{4}〕\d+号/.exec(title)?.[0] ?? null;
        const warnings = numbered ? [] : ["未识别到文号、发文机关和日期，可在列表中手动补充。"];
        if (format === "pdf" && parser === "builtin" && random() < 0.5) warnings.unshift("第 12–14 页是扫描图片，未能提取文字（需要 OCR）。");
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
          parser,
        };
        kbDocs = [doc, ...kbDocs];
        reports.push({ fileName, docId: doc.id, chunks: chunkCount, unchanged: false, warnings, error: null, parser });
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
    async addCommentReply(docId, commentId, text) {
      const doc = getDoc(docId);
      const body = text.trim();
      if (!body) throw new Error("回复内容不能为空");
      const target = doc.comments.find((c) => c.id === commentId);
      if (!target) throw new Error("找不到批注");
      const root = target.parentId ? (doc.comments.find((c) => c.id === target.parentId) ?? target) : target;
      const comments: CommentView[] = [
        ...doc.comments,
        {
          id: `reply-${Date.now().toString(36)}`,
          author: settings.fix.author,
          initials: [...settings.fix.author][0] ?? "A",
          date: new Date().toISOString(),
          text: body,
          parentId: root.id,
          done: false,
          blockIndex: root.blockIndex,
          paragraphIndex: root.paragraphIndex,
          quote: "",
        },
      ];
      return doc.commit("回复批注", { comments });
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
      const texts = edited ?? proposal.paragraphs.map((p) => p.new);
      if (texts.some((t) => t.includes(PLACEHOLDER)))
        throw new Error(edited ? "修改里还有“【待补充…】”，请改成实际内容后再应用" : "修改里有待补充的内容，请先查找资料、手动补充后再应用");
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
    pickFolder: async () => DEMO_FOLDER,
    kbCountImport: async (paths) => expandPaths(paths).length,
    async kbDocumentView(docId): Promise<KbDocumentView> {
      const doc = kbDocs.find((d) => d.id === docId);
      if (!doc) throw new Error("资料不存在");
      await delay(250);
      const lines: KbDocumentView["lines"] = [{ text: doc.title, headingLevel: null }];
      if (doc.meta.docNumber) lines.push({ text: doc.meta.docNumber, headingLevel: null });
      const passages = KB_PASSAGES.filter((p) => p.docId === docId);
      const sections = passages.length > 0 ? passages : [{ headingPath: ["第一章 总则", "第一条"], text: `为规范${doc.title.slice(0, 16)}相关工作，制定本文件。` }];
      const chunks: KbDocumentView["chunks"] = [];
      let offset = lines.reduce((n, l) => n + l.text.length + 1, 0);
      sections.forEach((p, i) => {
        const [chapter, article] = p.headingPath;
        lines.push({ text: chapter, headingLevel: /^第.+章/.test(chapter) ? 2 : 5 });
        offset += chapter.length + 1;
        const body = `${article ?? ""} ${p.text}`.trim();
        lines.push({ text: body, headingLevel: article && /^第.+条/.test(article) ? 4 : null });
        chunks.push({ chunkId: docId * 1000 + i, charStart: offset, charEnd: offset + body.length, headingPath: p.headingPath });
        offset += body.length + 1;
        const filler = "（演示内容）本条其余内容略。实际应用中这里显示导入时解析出的完整文字，表格按“列名：值”展开。";
        lines.push({ text: filler, headingLevel: null });
        offset += filler.length + 1;
      });
      return { document: clone(doc), lines, chunks };
    },
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
    async openUrl(url) {
      window.open(url, "_blank", "noopener");
    },
    async proofread(docId, options, range) {
      const doc = getDoc(docId);
      const count = doc.summary.paragraphCount;
      const [first, last] = range ?? [0, count - 1];
      const paragraphs: [number, string][] = [];
      for (let i = Math.max(0, first); i <= Math.min(last, count - 1); i++) {
        const p = doc.paragraph(i);
        if (p) paragraphs.push([i, paragraphText(p)]);
      }
      proofCancel = false;
      const emit = (stage: ProofProgress["stage"], done: number, total: number, found: ProofIssue[] = []) =>
        proofListeners.forEach((h) =>
          h({ docId, stage, done, total, found, paragraphs: Object.fromEntries(found.map((i) => [i.paragraph, paragraphs.find(([n]) => n === i.paragraph)?.[1] ?? ""])) }),
        );
      emit("rules", 0, 1);
      await delay(300);
      const sections = options.useModel ? Math.min(8, Math.ceil(paragraphs.length / 12)) : 0;
      if (options.useModel && !settings.roles.chat.model) throw new Error("请先在设置 › 模型分配中选择大语言模型，或关闭“使用大语言模型”只做规则检查");
      // Findings appear section by section, as the real backend reports them.
      const all = demoProofread(paragraphs, options).issues;
      emit("rules", 1, 1, all.filter((i) => i.source === "rule"));
      const byModel = all.filter((i) => i.source === "model");
      for (let i = 0; i < sections && !proofCancel; i++) {
        await delay(250);
        const per = Math.ceil(byModel.length / sections);
        emit("model", i + 1, sections, byModel.slice(i * per, (i + 1) * per));
      }
      if (options.categories.includes("citation")) {
        emit("citations", 0, 2);
        await delay(400);
      }
      emit("done", 1, 1);
      const report = demoProofread(paragraphs, options);
      return { ...report, cancelled: proofCancel };
    },
    async cancelProofread() {
      proofCancel = true;
    },
    onProofreadProgress(handler) {
      proofListeners.add(handler);
      return () => proofListeners.delete(handler);
    },
    async applyProofIssues(docId, issues) {
      const doc = getDoc(docId);
      await delay(120);
      const tracked = settings.fix.editMode === "tracked";
      const byParagraph = new Map<number, typeof issues>();
      for (const i of issues) if (i.suggestion !== null) byParagraph.set(i.paragraph, [...(byParagraph.get(i.paragraph) ?? []), i]);
      const paragraphs = new Map<number, SpanView[]>();
      const applied: string[] = [];
      for (const [index, list] of byParagraph) {
        const p = doc.paragraph(index);
        if (!p) continue;
        const current = paragraphText(p);
        const chars = [...current];
        let next = chars;
        // Last first, so earlier offsets stay valid.
        for (const i of [...list].sort((a, b) => b.start - a.start)) {
          if (next.slice(i.start, i.end).join("") !== i.original) continue;
          next = [...next.slice(0, i.start), ...(i.suggestion ?? ""), ...next.slice(i.end)];
          applied.push(i.id);
        }
        const text = next.join("");
        if (text !== current) paragraphs.set(index, rewriteSpans(p.spans, diffChars(current, text), tracked, settings.fix.author));
      }
      if (applied.length === 0) throw new Error("没有可以应用的修改：原文已变化，请重新校对");
      return { outcome: doc.commit("文档校对", { paragraphs }), applied };
    },
    async clearProofreadCache() {},

    async searchServices() {
      return SEARCH_SERVICES.map((s) => ({ ...s, hasKey: searchKeys.has(s.kind) }));
    },
    async setSearchKey(kind, key) {
      if (kind === "none") throw new Error("请先选择搜索服务");
      if (!key.trim()) throw new Error("Key 不能为空");
      searchKeys.add(kind);
      return SEARCH_SERVICES.map((s) => ({ ...s, hasKey: searchKeys.has(s.kind) }));
    },
    async clearSearchKey(kind) {
      searchKeys.delete(kind);
      return SEARCH_SERVICES.map((s) => ({ ...s, hasKey: searchKeys.has(s.kind) }));
    },

    async parserInfos() {
      return (["mineru", "paddleocr"] as const).map((k) => ({ ...PARSER_INFO[k], hasKey: parserKeys.has(k) }));
    },
    async setParserKey(kind, key) {
      if (kind === "builtin") throw new Error("普通模式不需要 Key");
      if (!key.trim()) throw new Error("请先填写 Key");
      parserKeys.add(kind);
      return { ...PARSER_INFO[kind], hasKey: true };
    },
    async clearParserKey(kind) {
      parserKeys.delete(kind);
    },
    async testParser(kind, key) {
      if (kind === "builtin") throw new Error("普通模式不需要 Key");
      await delay(700);
      const info = PARSER_INFO[kind];
      const base = { quota: null, quotaNote: info.quotaNote, consoleUrl: info.consoleUrl };
      if (!key && !parserKeys.has(kind)) return { ok: false, message: "请先填写 Key", ...base };
      if (key && key.trim().length < 8) return { ok: false, message: "Key 无效：401 Unauthorized", ...base };
      return { ok: true, message: `连接正常，${info.name} Key 有效`, ...base };
    },

    async exportBackup() {
      await backupSteps(["导出审稿人和案例", "复制知识库", "打包原文件"]);
      const m = backupManifest();
      return { path: "D:\\备份\\AutoPassDoc-备份.apdbak", size: 18_400_000, manifest: m, missingFiles: [] };
    },
    pickBackup: async () => "D:\\备份\\AutoPassDoc-备份.apdbak",
    async inspectBackup() {
      await delay(200);
      return { ...backupManifest(), createdAt: unixNow() - 3 * 86_400, documents: kbDocs.length + 2 };
    },
    async importBackup(_path, mode) {
      await backupSteps(mode === "replace" ? ["自动备份当前数据", "还原审稿人", "还原知识库"] : ["合并审稿人", "合并案例", "合并知识库"]);
      const m = backupManifest();
      return {
        mode,
        manifest: m,
        reviewersAdded: mode === "replace" ? reviewers.length : 0,
        reviewersSkipped: mode === "replace" ? 0 : reviewers.length,
        casesAdded: mode === "replace" ? cases.length : 2,
        casesSkipped: mode === "replace" ? 0 : cases.length - 2,
        profilesAdded: 0,
        documentsAdded: 2,
        documentsSkipped: mode === "replace" ? 0 : kbDocs.length,
        autoBackup: mode === "replace" ? "C:\\Users\\演示\\AppData\\Roaming\\AutoPassDoc\\backups\\自动备份.apdbak" : null,
      };
    },
    onBackupProgress(handler) {
      backupListeners.add(handler);
      return () => backupListeners.delete(handler);
    },
    appInfo: async () => ({ version: "0.3.0", dataDir: "C:\\Users\\演示\\AppData\\Roaming\\AutoPassDoc" }),
    async checkUpdate() {
      await delay(600);
      return {
        current: "0.3.0",
        latest: "0.3.1",
        hasUpdate: true,
        name: "AutoPassDoc 0.3.1",
        notes: "### 修复\n- 文档校对：修正跨页编号检查的误报。\n- 知识库：MinerU 解析大文件时的超时。",
        url: "https://github.com/cyDione/AutoPassDoc/releases",
        publishedAt: new Date(Date.now() - 2 * 86_400_000).toISOString(),
        assets: [demoInstaller],
        installer: demoInstaller,
      };
    },
    async downloadUpdate(installer) {
      updateCancelled = false;
      for (let done = 0; done <= installer.size; done += installer.size / 25) {
        if (updateCancelled) throw new Error("已取消下载");
        for (const l of updateListeners) l({ downloaded: Math.round(done), total: installer.size });
        await delay(120);
      }
      return `C:\\Users\\演示\\AppData\\Local\\Temp\\AutoPassDoc-update\\${installer.name}`;
    },
    async cancelUpdateDownload() {
      updateCancelled = true;
    },
    async installUpdate() {
      await delay(400);
      throw new Error("演示模式不会真的安装更新");
    },
    onUpdateProgress(handler) {
      updateListeners.add(handler);
      return () => updateListeners.delete(handler);
    },

    async webSearch({ query, need }) {
      const q = (query ?? need ?? "").trim();
      if (!q) throw new Error("请输入要查找的内容");
      await delay(900);
      const model = !!settings.roles.chat.model;
      const terms = query ? [query.trim()] : model ? [`上海市 ${q.slice(0, 10)}`, `${q.slice(0, 8)} 文号`] : [q];
      const site = "https://tjj.sh.gov.cn";
      return {
        via: "local",
        queries: terms,
        answer: model && need ? { text: "《上海市国民经济和社会发展统计公报》（2024年）", quote: "年末全市常住人口2480.26万人。", title: "2024年上海市国民经济和社会发展统计公报", url: `${site}/tjgb/20250319/2024gb.html` } : null,
        notes: model ? [] : ["尚未配置大语言模型，无法由 AI 生成搜索词和筛选结果"],
        results: [
          {
            title: `2024年上海市国民经济和社会发展统计公报（${q.slice(0, 12)}）`,
            url: `${site}/tjgb/20250319/2024gb.html`,
            site: "tjj.sh.gov.cn",
            snippet: "年末全市常住人口2480.26万人。全年地区生产总值53926.71亿元，比上年增长5.0%。",
            kind: "page",
            fileType: null,
            importable: false,
            trusted: true,
            reason: model ? "公报原文，含常住人口和地区生产总值" : null,
          },
          {
            title: "2024年上海市国民经济和社会发展统计公报（PDF 全文）",
            url: `${site}/tjgb/20250319/2024gb.pdf`,
            site: "tjj.sh.gov.cn",
            snippet: "附件，来自：2024年上海市国民经济和社会发展统计公报",
            kind: "file",
            fileType: "pdf",
            importable: true,
            trusted: true,
            reason: model ? "公报 PDF 全文，可存入知识库" : null,
          },
          {
            title: "崇明区统计年鉴2024（附表）",
            url: "https://www.shcm.gov.cn/tjj/nianjian2024.xls",
            site: "www.shcm.gov.cn",
            snippet: "附件，来自：崇明区统计年鉴",
            kind: "file",
            fileType: "xls",
            importable: false,
            trusted: true,
            reason: null,
          },
          {
            title: `解读：${q.slice(0, 12)}`,
            url: "https://www.example.com/news/2024gb.html",
            site: "www.example.com",
            snippet: "媒体对统计公报的解读文章。",
            kind: "page",
            fileType: null,
            importable: false,
            trusted: false,
            reason: model ? "转述公报数据，需以原文为准" : null,
          },
        ],
      };
    },
    async webSavePageToKb(url) {
      const name = `${decodeURIComponent(url.split("/").pop() ?? "网页").replace(/\.html?$/, "")}.md`;
      const [report] = await importFiles([name]);
      return report;
    },
    async webDownloadToKb(url) {
      const name = decodeURIComponent(url.split("/").pop() ?? "下载的资料.pdf");
      const [report] = await importFiles([name]);
      return report;
    },

    confirm: async (message) => window.confirm(message),
    async askSave(message) {
      if (window.confirm(`${message}\n\n确定：保存；取消：选择是否放弃修改`)) return "save";
      return window.confirm("不保存，直接关闭？") ? "discard" : "cancel";
    },
    onCloseRequested: () => () => {},
  };
}
