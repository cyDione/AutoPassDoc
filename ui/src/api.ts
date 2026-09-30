import type {
  AppInfo,
  AuthorView,
  BackupExport,
  BackupImport,
  BackupImportMode,
  BackupManifest,
  BackupProgress,
  BlockView,
  Case,
  DocState,
  EditOutcome,
  FixProgress,
  FixProposal,
  FixRequest,
  KbDocument,
  KbDocumentView,
  KbHit,
  KbImportReport,
  KbMeta,
  KbProgress,
  KbStats,
  ModelProfileOverride,
  ModelRoleName,
  ModelView,
  OpenedDoc,
  ParserInfo,
  ParserKind,
  ParserTest,
  PreReviewItem,
  ProofIssue,
  ProofOptions,
  ProofProgress,
  ProofReport,
  ProbeResult,
  ProviderView,
  Reviewer,
  ReviewerProfile,
  Settings,
  Summary,
  UpdateInfo,
  SearchService,
  SearchServiceInfo,
  WebLookup,
  WebSearchOutcome,
} from "./types";

export type Unsubscribe = () => void;

/** Everything the UI needs from the Rust side. */
export interface Backend {
  /** Shows a file picker and opens the chosen document; `null` if cancelled. */
  pickAndOpen(): Promise<OpenedDoc | null>;
  open(path: string): Promise<OpenedDoc>;
  blocks(docId: number, start: number, end: number): Promise<BlockView[]>;
  /** Asks where to save; returns the saved path, or `null` if cancelled. */
  saveAs(docId: number): Promise<string | null>;
  close(docId: number): Promise<void>;
  /** A document passed on the command line at startup, if any. */
  initialFile(): Promise<string | null>;
  imageUrl(docId: number, relId: string): string;
  /** Subscribes to files dropped on the window; returns an unsubscribe function. */
  onFileDrop(handler: (paths: string[]) => void, onHover: (hovering: boolean) => void): () => void;

  // Editing
  docState(docId: number): Promise<DocState>;
  summary(docId: number): Promise<Summary>;
  undo(docId: number): Promise<EditOutcome>;
  redo(docId: number): Promise<EditOutcome>;
  /** Saves to the path of the last save; rejects if the document was never saved. */
  save(docId: number): Promise<DocState>;
  setCommentDone(docId: number, commentId: string, done: boolean): Promise<EditOutcome>;
  /** Adds a Word reply to the comment's thread, signed with the revision author from the settings. */
  addCommentReply(docId: number, commentId: string, text: string): Promise<EditOutcome>;

  // AI fixes
  /**
   * Runs one fix; progress also arrives through onFixProgress. `request.selection`
   * replaces the paragraphs under the comment's highlight.
   */
  fixComment(docId: number, commentId: string, request?: FixRequest): Promise<FixProposal>;
  /** Starts fixes for several comments; each result arrives through onFixProgress. */
  fixBatch(docId: number, commentIds: string[]): Promise<void>;
  onFixProgress(handler: (p: FixProgress) => void): Unsubscribe;
  /** Applies a proposal, optionally with user-edited paragraph texts. `force` applies below the threshold. */
  applyFix(docId: number, proposalId: string, edited: string[] | null, force: boolean): Promise<EditOutcome>;
  rejectFix(proposalId: string): Promise<void>;

  // Reviewers
  documentAuthors(docId: number): Promise<AuthorView[]>;
  /**
   * Maps a comment signature to an existing reviewer (`reviewerId`), a new one
   * (`newReviewer`), or clears it (both null). `writeBack` also renames the
   * author inside the document.
   */
  assignAuthor(
    docId: number,
    author: string,
    initials: string,
    reviewerId: number | null,
    newReviewer: string | null,
    writeBack: boolean,
  ): Promise<{ authors: AuthorView[]; outcome: EditOutcome | null }>;
  reviewers(): Promise<Reviewer[]>;
  createReviewer(name: string, note: string): Promise<Reviewer>;
  updateReviewer(id: number, name: string, note: string, threshold: number | null): Promise<Reviewer>;
  deleteReviewer(id: number): Promise<void>;
  mergeReviewers(from: number, into: number): Promise<void>;
  reviewerProfile(id: number): Promise<ReviewerProfile | null>;
  /** Re-distils the reviewer's profile from their cases now. */
  distillProfile(id: number): Promise<ReviewerProfile>;
  reviewerCases(id: number): Promise<Case[]>;
  /** Comments this reviewer would likely make on paragraphs start..end (exclusive). */
  preReview(docId: number, reviewerId: number, start: number, end: number): Promise<PreReviewItem[]>;
  /** Asks where to save and writes the decided cases as JSONL for judge training; `null` if cancelled. */
  exportDataset(): Promise<string | null>;

  // Settings and models
  settings(): Promise<Settings>;
  saveSettings(settings: Settings): Promise<Settings>;
  providers(): Promise<ProviderView[]>;
  /** `apiKey`: a new key, `null` to remove the key, `undefined` to keep it. */
  saveProvider(provider: Omit<ProviderView, "hasKey">, apiKey?: string | null): Promise<ProviderView>;
  deleteProvider(id: string): Promise<void>;
  /** Fetches the provider's model list from its API and caches it. */
  fetchModels(providerId: string): Promise<ModelView[]>;
  /** The cached model list. */
  providerModels(providerId: string): Promise<ModelView[]>;
  setModelProfile(providerId: string, modelId: string, profile: ModelProfileOverride | null): Promise<ModelView>;
  testRole(role: ModelRoleName): Promise<ProbeResult>;

  // Knowledge base
  kbDocuments(): Promise<KbDocument[]>;
  kbStats(): Promise<KbStats>;
  /** Imports the given files, or asks for files when `paths` is omitted. */
  kbImport(paths?: string[]): Promise<KbImportReport[]>;
  /** Asks for a folder; null when cancelled. */
  pickFolder(): Promise<string | null>;
  /** How many files importing these paths would read, folders expanded. */
  kbCountImport(paths: string[]): Promise<number>;
  kbDocumentView(docId: number): Promise<KbDocumentView>;
  kbRemove(docId: number): Promise<void>;
  kbUpdateMeta(docId: number, meta: KbMeta): Promise<KbDocument>;
  kbSearch(text: string): Promise<KbHit[]>;
  /** Starts embedding chunks that lack vectors; progress arrives through onKbProgress. */
  kbEmbed(): Promise<void>;
  /** Deletes the vectors of the configured embedding model, e.g. after switching to a different model with the same name. */
  kbClearEmbeddings(): Promise<void>;
  onKbProgress(handler: (p: KbProgress) => void): Unsubscribe;
  /** Opens a file with the system's default app. */
  openPath(path: string): Promise<void>;
  /** Opens a web page in the default browser. */
  openUrl(url: string): Promise<void>;

  // Proofreading
  /** Proofreads the document, or paragraphs `range` (first and last, inclusive). */
  proofread(docId: number, options: ProofOptions, range: [number, number] | null): Promise<ProofReport>;
  cancelProofread(): Promise<void>;
  onProofreadProgress(handler: (p: ProofProgress) => void): Unsubscribe;
  /** Writes the issues' suggestions into the document as one undo step. */
  applyProofIssues(docId: number, issues: ProofIssue[]): Promise<{ outcome: EditOutcome; applied: string[] }>;
  clearProofreadCache(): Promise<void>;

  // Enhanced parsing (MinerU / PaddleOCR)
  parserInfos(): Promise<ParserInfo[]>;
  setParserKey(kind: ParserKind, key: string): Promise<ParserInfo>;
  clearParserKey(kind: ParserKind): Promise<void>;
  /** Tests `key` when given, else the saved key. */
  testParser(kind: ParserKind, key?: string): Promise<ParserTest>;

  // Backup and about
  /** Asks where to save, then writes a backup of the knowledge base and reviewers; null when cancelled. */
  exportBackup(): Promise<BackupExport | null>;
  /** Asks for a backup file; null when cancelled. */
  pickBackup(): Promise<string | null>;
  inspectBackup(path: string): Promise<BackupManifest>;
  importBackup(path: string, mode: BackupImportMode): Promise<BackupImport>;
  onBackupProgress(handler: (p: BackupProgress) => void): Unsubscribe;
  appInfo(): Promise<AppInfo>;
  checkUpdate(): Promise<UpdateInfo>;

  // Web
  /** Searches the web: the chat model's own search first, else whitelisted sites from this machine. */
  webSearch(request: WebLookup): Promise<WebSearchOutcome>;
  /** Downloads a file from the last search results and imports it into the knowledge base. */
  webDownloadToKb(url: string): Promise<KbImportReport>;
  /** Search APIs (智谱 / 博查 / Tavily) and whether each has a key. */
  searchServices(): Promise<SearchServiceInfo[]>;
  setSearchKey(kind: SearchService, key: string): Promise<SearchServiceInfo[]>;
  clearSearchKey(kind: SearchService): Promise<SearchServiceInfo[]>;

  // Window
  /** Asks the user to confirm a destructive step; resolves true to go ahead. */
  confirm(message: string, title: string, okLabel: string): Promise<boolean>;
  /** Asks whether to save unsaved changes before they would be lost. */
  askSave(message: string, title: string): Promise<"save" | "discard" | "cancel">;
  /** Runs `allow` when the user closes the window; the window stays open when it resolves false. */
  onCloseRequested(allow: () => Promise<boolean>): Unsubscribe;
}

const isTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

async function tauriBackend(): Promise<Backend> {
  const { invoke, convertFileSrc } = await import("@tauri-apps/api/core");
  const dialog = await import("@tauri-apps/plugin-dialog");
  const opener = await import("@tauri-apps/plugin-opener");
  const { getCurrentWebview } = await import("@tauri-apps/api/webview");
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  const { listen } = await import("@tauri-apps/api/event");

  const subscribe = <T,>(event: string, handler: (payload: T) => void): Unsubscribe => {
    let unlisten: (() => void) | null = null;
    let cancelled = false;
    listen<T>(event, (e) => handler(e.payload)).then((fn) => (cancelled ? fn() : (unlisten = fn)));
    return () => {
      cancelled = true;
      unlisten?.();
    };
  };

  const open = (path: string) => invoke<OpenedDoc>("open_document", { path });

  return {
    open,
    async pickAndOpen() {
      const path = await dialog.open({
        multiple: false,
        directory: false,
        filters: [{ name: "Word 文档", extensions: ["docx"] }],
      });
      return path ? open(path) : null;
    },
    blocks: (docId, start, end) => invoke<BlockView[]>("get_blocks", { docId, start, end }),
    async saveAs(docId) {
      const defaultPath = await invoke<string>("suggested_save_path", { docId });
      const path = await dialog.save({
        defaultPath,
        filters: [{ name: "Word 文档", extensions: ["docx"] }],
      });
      if (!path) return null;
      await invoke<DocState>("save_document_as", { docId, path });
      return path;
    },
    close: (docId) => invoke("close_document", { docId }),
    initialFile: () => invoke<string | null>("initial_file"),
    imageUrl: (docId, relId) => convertFileSrc(`${docId}/${relId}`, "apd"),
    onFileDrop(handler, onHover) {
      let unlisten: (() => void) | null = null;
      let cancelled = false;
      getCurrentWebview()
        .onDragDropEvent((event) => {
          const p = event.payload;
          if (p.type === "enter" || p.type === "over") onHover(true);
          else if (p.type === "leave") onHover(false);
          else if (p.type === "drop") {
            onHover(false);
            handler(p.paths);
          }
        })
        .then((fn) => (cancelled ? fn() : (unlisten = fn)));
      return () => {
        cancelled = true;
        unlisten?.();
      };
    },

    docState: (docId) => invoke("doc_state", { docId }),
    summary: (docId) => invoke("document_summary", { docId }),
    undo: (docId) => invoke("undo", { docId }),
    redo: (docId) => invoke("redo", { docId }),
    save: (docId) => invoke("save_document", { docId }),
    setCommentDone: (docId, commentId, done) => invoke("set_comment_done", { docId, commentId, done }),
    addCommentReply: (docId, commentId, text) => invoke("add_comment_reply", { docId, commentId, text }),

    fixComment: (docId, commentId, request) => invoke("fix_comment", { docId, commentId, request: request ?? null }),
    fixBatch: (docId, commentIds) => invoke("fix_batch", { docId, commentIds }),
    onFixProgress: (handler) => subscribe<FixProgress>("fix-progress", handler),
    applyFix: (docId, proposalId, edited, force) => invoke("apply_fix", { docId, proposalId, edited, force }),
    rejectFix: (proposalId) => invoke("reject_fix", { proposalId }),

    documentAuthors: (docId) => invoke("document_authors", { docId }),
    assignAuthor: (docId, author, initials, reviewerId, newReviewer, writeBack) =>
      invoke("assign_author", { docId, author, initials, reviewerId, newReviewer, writeBack }),
    reviewers: () => invoke("list_reviewers"),
    createReviewer: (name, note) => invoke("create_reviewer", { name, note }),
    updateReviewer: (id, name, note, threshold) => invoke("update_reviewer", { id, name, note, threshold }),
    deleteReviewer: (id) => invoke("delete_reviewer", { id }),
    mergeReviewers: (from, into) => invoke("merge_reviewers", { from, into }),
    reviewerProfile: (id) => invoke("reviewer_profile", { id }),
    distillProfile: (id) => invoke("distill_profile", { id }),
    reviewerCases: (id) => invoke("reviewer_cases", { id }),
    preReview: (docId, reviewerId, start, end) => invoke("pre_review", { docId, reviewerId, start, end }),
    async exportDataset() {
      const path = await dialog.save({
        defaultPath: "autopassdoc-judge-dataset.jsonl",
        filters: [{ name: "JSON Lines", extensions: ["jsonl"] }],
      });
      if (!path) return null;
      await invoke("export_dataset", { path });
      return path;
    },

    settings: () => invoke("get_settings"),
    saveSettings: (settings) => invoke("save_settings", { settings }),
    providers: () => invoke("list_providers"),
    saveProvider: (provider, apiKey) =>
      invoke("save_provider", { provider, apiKey: apiKey ?? null, keepKey: apiKey === undefined }),
    deleteProvider: (id) => invoke("delete_provider", { id }),
    fetchModels: (providerId) => invoke("fetch_models", { providerId }),
    providerModels: (providerId) => invoke("provider_models", { providerId }),
    setModelProfile: (providerId, modelId, profile) => invoke("set_model_profile", { providerId, modelId, profile }),
    testRole: (role) => invoke("test_role", { role }),

    kbDocuments: () => invoke("kb_documents"),
    kbStats: () => invoke("kb_stats"),
    async kbImport(paths) {
      let files = paths;
      if (!files) {
        const picked = await dialog.open({
          multiple: true,
          directory: false,
          filters: [{ name: "资料文件", extensions: ["docx", "pdf", "txt", "md", "markdown", "png", "jpg", "jpeg", "bmp", "tif", "tiff", "webp"] }],
        });
        if (!picked) return [];
        files = Array.isArray(picked) ? picked : [picked];
      }
      return invoke<KbImportReport[]>("kb_import", { paths: files });
    },
    async pickFolder() {
      const picked = await dialog.open({ multiple: false, directory: true });
      return typeof picked === "string" ? picked : null;
    },
    kbCountImport: (paths) => invoke("kb_count_import", { paths }),
    kbDocumentView: (docId) => invoke("kb_document_view", { docId }),
    kbRemove: (docId) => invoke("kb_remove", { docId }),
    kbUpdateMeta: (docId, meta) => invoke("kb_update_meta", { docId, meta }),
    kbSearch: (text) => invoke("kb_search", { text }),
    kbEmbed: () => invoke("kb_embed"),
    kbClearEmbeddings: () => invoke("kb_clear_embeddings"),
    onKbProgress: (handler) => subscribe<KbProgress>("kb-progress", handler),
    openPath: (path) => opener.openPath(path),
    openUrl: (url) => opener.openUrl(url),
    proofread: (docId, options, range) => invoke("proofread", { docId, options, range }),
    cancelProofread: () => invoke("cancel_proofread"),
    onProofreadProgress: (handler) => subscribe<ProofProgress>("proofread-progress", handler),
    applyProofIssues: (docId, issues) => invoke("apply_proof_issues", { docId, issues }),
    clearProofreadCache: () => invoke("clear_proofread_cache"),
    parserInfos: () => invoke("parser_infos"),
    setParserKey: (kind, key) => invoke("set_parser_key", { kind, key }),
    clearParserKey: (kind) => invoke("clear_parser_key", { kind }),
    testParser: (kind, key) => invoke("test_parser", { kind, key: key ?? null }),
    async exportBackup() {
      const stamp = new Date().toISOString().slice(0, 10).replaceAll("-", "");
      const path = await dialog.save({
        defaultPath: `AutoPassDoc-备份-${stamp}.apdbak`,
        filters: [{ name: "AutoPassDoc 备份", extensions: ["apdbak", "zip"] }],
      });
      if (!path) return null;
      return invoke<BackupExport>("export_backup", { path });
    },
    async pickBackup() {
      const picked = await dialog.open({
        multiple: false,
        directory: false,
        filters: [{ name: "AutoPassDoc 备份", extensions: ["apdbak", "zip"] }],
      });
      return typeof picked === "string" ? picked : null;
    },
    inspectBackup: (path) => invoke("inspect_backup", { path }),
    importBackup: (path, mode) => invoke("import_backup", { path, mode }),
    onBackupProgress: (handler) => subscribe<BackupProgress>("backup-progress", handler),
    appInfo: () => invoke("app_info"),
    checkUpdate: () => invoke("check_update"),
    webSearch: (request) => invoke("web_search", { request }),
    searchServices: () => invoke("search_services"),
    setSearchKey: (kind, key) => invoke("set_search_key", { kind, key }),
    clearSearchKey: (kind) => invoke("clear_search_key", { kind }),
    webDownloadToKb: (url) => invoke("web_download_to_kb", { url }),

    confirm: (message, title, okLabel) =>
      dialog.confirm(message, { title, kind: "warning", okLabel, cancelLabel: "取消" }),
    async askSave(message, title) {
      const buttons = { yes: "保存", no: "不保存", cancel: "取消" };
      const choice = await dialog.message(message, { title, kind: "warning", buttons });
      // Custom buttons come back as their labels on some platforms and as Yes/No on others.
      if (choice === "Yes" || choice === buttons.yes) return "save";
      if (choice === "No" || choice === buttons.no) return "discard";
      return "cancel";
    },
    onCloseRequested(allow) {
      let unlisten: (() => void) | null = null;
      let cancelled = false;
      getCurrentWindow()
        .onCloseRequested(async (event) => {
          if (!(await allow())) event.preventDefault();
        })
        .then((fn) => (cancelled ? fn() : (unlisten = fn)));
      return () => {
        cancelled = true;
        unlisten?.();
      };
    },
  };
}

/**
 * Browser-only backend for developing the UI without Tauri: serves a generated
 * sample report exported by `pnpm demo-data` and fakes everything else in
 * memory. Loaded lazily so it stays out of the desktop app.
 */
async function demoBackend(): Promise<Backend> {
  const { createDemoBackend } = await import("./demo");
  return createDemoBackend();
}

export const backend: Promise<Backend> = isTauri ? tauriBackend() : demoBackend();
