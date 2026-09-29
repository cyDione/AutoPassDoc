// Mirrors docx-engine's view structs (crates/docx-engine/src/view.rs).

export interface Summary {
  blockCount: number;
  paragraphCount: number;
  charCount: number;
  outline: OutlineItem[];
  comments: CommentView[];
}

export interface OutlineItem {
  blockIndex: number;
  paragraphIndex: number;
  level: number;
  text: string;
}

export interface CommentView {
  id: string;
  author: string;
  initials: string | null;
  date: string | null;
  text: string;
  parentId: string | null;
  done: boolean;
  blockIndex: number | null;
  paragraphIndex: number | null;
  quote: string;
}

export type Inline =
  | { type: "text" }
  | { type: "image"; rel_id: string | null }
  | { type: "note"; id: string }
  | { type: "math" };

export interface SpanView {
  text: string;
  bold: boolean;
  italic: boolean;
  underline: boolean;
  strike: boolean;
  superscript: boolean;
  subscript: boolean;
  revision: "none" | "insert" | "delete";
  revisionAuthor?: string;
  inline: Inline;
  commentIds?: string[];
}

export interface ParagraphView {
  index: number;
  headingLevel: number | null;
  listLabel: string | null;
  align: string | null;
  spans: SpanView[];
}

export interface CellView {
  gridSpan: number;
  vMerge: boolean | null;
  paragraphs: ParagraphView[];
}

export type BlockView =
  | { kind: "paragraph"; index: number; paragraph: ParagraphView }
  | { kind: "table"; index: number; rows: CellView[][] };

export interface OpenedDoc {
  docId: number;
  path: string;
  fileName: string;
  summary: Summary;
}

// ---- Editing, AI fixes, reviewers, models and knowledge base ----
// Mirrors crates/app-core and src-tauri/src/lib.rs.

/** Undo/redo and save state of an open document. */
export interface DocState {
  dirty: boolean;
  undoLabel: string | null;
  redoLabel: string | null;
  /** Where the document was last saved in this session, if anywhere. */
  savedPath: string | null;
}

/** Result of any command that changes the document. */
export interface EditOutcome {
  state: DocState;
  summary: Summary;
}

export type ThinkingLevel = "off" | "low" | "medium" | "high";
export type ModelRoleName = "chat" | "decision" | "embedding" | "rerank";

export interface DiffSpan {
  kind: "equal" | "insert" | "delete";
  text: string;
}

export interface FixParagraph {
  /** Paragraph index in the document. */
  index: number;
  old: string;
  new: string;
  diff: DiffSpan[];
}

/** A knowledge-base passage the fix relied on. */
export interface Citation {
  n: number;
  docId: number;
  chunkId: number;
  fileName: string;
  title: string;
  headingPath: string[];
  text: string;
  storedPath: string;
  charStart: number;
  charEnd: number;
}

export interface JudgeItem {
  key: string;
  /** e.g. "回应批注", "保持原意", "有据可查", "公文规范" */
  label: string;
  /** Probability (0..1) of the good outcome. */
  value: number;
  weight: number;
  /** A hard item must pass on its own for one-click apply. */
  hard: boolean;
  passed: boolean;
}

export interface Judgement {
  confidence: number;
  threshold: number;
  passed: boolean;
  items: JudgeItem[];
  /** Comment category: 数据口径 / 政策依据 / 措辞规范 / 格式 / 逻辑结构 / 其他 */
  category: string | null;
  backend: "jev" | "chat";
  model: string;
}

/** Paragraphs the user selected by hand for an AI fix, when the reviewer's highlight missed some. */
export interface FixSelection {
  startParagraph: number;
  endParagraph: number;
  text: string;
}

export interface FixProposal {
  id: string;
  docId: number;
  commentId: string;
  /** Reviewer the comment author maps to. */
  reviewer: string | null;
  paragraphs: FixParagraph[];
  explanation: string;
  citations: Citation[];
  /** `null` when no decision model is set up or judging failed (see judgeError). */
  judge: Judgement | null;
  judgeError: string | null;
  model: string;
  elapsedMs: number;
  warnings: string[];
  context: { passages: number; examples: number; profile: boolean };
}

export type FixStage = "context" | "retrieve" | "generate" | "judge" | "done" | "error";

/** Emitted while a fix (single or batch) runs. */
export interface FixProgress {
  docId: number;
  commentId: string;
  stage: FixStage;
  message?: string;
  proposal?: FixProposal;
  error?: string;
}

export interface Reviewer {
  id: number;
  name: string;
  note: string;
  threshold: number | null;
  caseCount: number;
  profileVersion: number | null;
}

/** A comment signature in the open document and the reviewer it maps to. */
export interface AuthorView {
  author: string;
  initials: string;
  commentCount: number;
  reviewer: Reviewer | null;
  /** Machine defaults like "Administrator": mappings apply to this document only. */
  generic: boolean;
}

export interface ReviewerProfile {
  reviewerId: number;
  version: number;
  /** Unix seconds. */
  createdAt: number;
  caseCount: number;
  summary: string;
  focus: string[];
  preferences: string[];
  commonRequests: string[];
}

export interface Case {
  id: number;
  reviewerId: number | null;
  docName: string;
  commentId: string;
  author: string;
  comment: string;
  original: string;
  suggestion: string;
  finalText: string | null;
  action: "pending" | "accepted" | "edited" | "rejected";
  confidence: number | null;
  category: string | null;
  /** Unix seconds. */
  createdAt: number;
}

export interface RoleModel {
  providerId: string;
  model: string;
  /** Empty = the model's default. */
  thinking: "" | ThinkingLevel;
}

export interface Settings {
  roles: {
    chat: RoleModel;
    decision: RoleModel;
    decisionBackend: "jev" | "chat";
    embedding: RoleModel;
    rerank: RoleModel;
  };
  fix: {
    threshold: number;
    editMode: "tracked" | "direct";
    author: string;
    resolveOnApply: boolean;
    replyOnApply: boolean;
    replyText: string;
    useKb: boolean;
    kbPassages: number;
    concurrency: number;
    profileEvery: number;
  };
}

export type ProviderKind = "openai" | "openrouter" | "anthropic" | "ollama";

export interface ProviderView {
  id: string;
  name: string;
  kind: ProviderKind;
  baseUrl: string;
  decisionPath: string | null;
  rerankPath: string | null;
  hasKey: boolean;
}

export interface ModelProfileView {
  contextWindow: number;
  maxOutputTokens: number;
  /** Thinking levels the UI may offer for this model. */
  levels: ThinkingLevel[];
  reasoning: "none" | "effort" | "toggle" | "budget" | "always";
  jsonMode: boolean;
  source: "fetched" | "builtin" | "manual" | "default";
}

export interface ModelView {
  id: string;
  roleHint: ModelRoleName;
  profile: ModelProfileView;
  /** The profile has user overrides. */
  manual: boolean;
}

/** User overrides for a model's capabilities; omitted fields keep the detected value. */
export interface ModelProfileOverride {
  contextWindow?: number;
  maxOutputTokens?: number;
  reasoning?: ModelProfileView["reasoning"];
  jsonMode?: boolean;
}

export interface ProbeResult {
  ok: boolean;
  latencyMs: number;
  summary: string;
}

export interface KbMeta {
  title: string | null;
  docNumber: string | null;
  issuer: string | null;
  date: string | null;
}

export interface KbDocument {
  id: number;
  title: string;
  fileName: string;
  storedPath: string;
  originalPath: string;
  format: string;
  meta: KbMeta;
  chunkCount: number;
  charCount: number;
  /** Unix seconds. */
  importedAt: number;
  sha256: string;
  /** Problems from the last import, e.g. a scanned PDF that needs OCR. */
  warnings: string[];
}

export interface KbImportReport {
  fileName: string;
  docId: number | null;
  chunks: number;
  unchanged: boolean;
  warnings: string[];
  error: string | null;
}

export interface KbHit {
  chunkId: number;
  docId: number;
  fileName: string;
  title: string;
  storedPath: string;
  headingPath: string[];
  text: string;
  parentText: string;
  charStart: number;
  charEnd: number;
  keywordRank: number | null;
  vectorRank: number | null;
  score: number;
  /** Set when a reranker is configured. */
  rerankScore: number | null;
}

export interface KbStats {
  documents: number;
  chunks: number;
  /** Chunks embedded with the configured embedding model. */
  embedded: number;
  embeddingModel: string | null;
}

export interface KbProgress {
  stage: "import" | "embed";
  done: number;
  total: number;
  current?: string;
  message?: string;
}

/** A comment the reviewer would likely make, from 按审稿人预审. */
export interface PreReviewItem {
  paragraphIndex: number;
  quote: string;
  comment: string;
  category: string | null;
}
