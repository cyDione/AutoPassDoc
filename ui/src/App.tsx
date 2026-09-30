import { useCallback, useEffect, useMemo, useRef, useState, type CSSProperties } from "react";
import { FolderOpen } from "lucide-react";
import { backend as backendPromise, type Backend } from "./api";
import { CommentsPanel } from "./components/CommentsPanel";
import { DocumentView, type MarginHost, type ScrollTarget } from "./components/DocumentView";
import type { Section } from "./components/reviewers/PreReviewCard";
import { SettingsDialog, type SettingsTab } from "./components/settings/SettingsDialog";
import { Sidebar, type Page } from "./components/Sidebar";
import { TopBar } from "./components/TopBar";
import { WebSearchDialog, type WebSearchRequest } from "./components/WebSearchDialog";
import { useDocumentSession } from "./hooks/useDocumentSession";
import { usePanelWidth } from "./hooks/usePanelWidth";
import { useCommentLayout } from "./hooks/useCommentLayout";
import { useTheme } from "./hooks/useTheme";
import { KnowledgeBasePage, type DroppedFiles } from "./pages/KnowledgeBasePage";
import { ProofreadPage } from "./pages/ProofreadPage";
import { ReviewersPage } from "./pages/ReviewersPage";
import type { CommentView, FixSelection, FixSource, OpenedDoc, OutlineItem, Settings } from "./types";
import { errorMessage, isTyping } from "./util";

interface Toast {
  text: string;
  error?: boolean;
}

const PAGE_TITLE: Record<Page, string | null> = { doc: null, proofread: "文档校对", kb: "知识库", reviewers: "审稿人" };

export default function App() {
  const [backend, setBackend] = useState<Backend | null>(null);
  const [toast, setToast] = useState<Toast | null>(null);
  const notify = useCallback((text: string, error?: boolean) => setToast({ text, error }), []);
  const { doc, docState, version, authors, setAuthors, loading, load, edit, undo, redo, save, saveAs, close, saveAndClose } = useDocumentSession(backend, notify);
  const [theme, setTheme] = useTheme();
  const [commentLayout, setCommentLayout] = useCommentLayout();
  const marginLayout = commentLayout === "margin";
  /** The page margin that holds the comment cards in the classic Word layout (C-4). */
  const [marginHost, setMarginHost] = useState<MarginHost | null>(null);
  const [settings, setSettings] = useState<Settings | null>(null);
  const [settingsTab, setSettingsTab] = useState<SettingsTab | null>(null);
  const [page, setPage] = useState<Page>("doc");
  /** Pages stay mounted after their first visit so running imports keep reporting progress. */
  const [visited, setVisited] = useState<ReadonlySet<Page>>(() => new Set(["doc"]));
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const [panelOpen, setPanelOpen] = useState(true);
  const [activeCommentId, setActiveCommentId] = useState<string | null>(null);
  const [target, setTarget] = useState<ScrollTarget | null>(null);
  const [topBlock, setTopBlock] = useState(0);
  const [dragging, setDragging] = useState(false);
  const [kbDrop, setKbDrop] = useState<DroppedFiles | null>(null);
  const [docSelection, setDocSelection] = useState<FixSelection | null>(null);
  const [webSearch, setWebSearch] = useState<WebSearchRequest | null>(null);
  const panelWidth = usePanelWidth();
  const onSelectionChange = useCallback(
    (next: FixSelection | null) =>
      setDocSelection((prev) =>
        prev === next ||
        (prev && next && prev.startParagraph === next.startParagraph && prev.endParagraph === next.endParagraph && prev.text === next.text)
          ? prev
          : next,
      ),
    [],
  );

  // A selection made for one comment must not carry over to the next (C-1).
  const lastActive = useRef(activeCommentId);
  useEffect(() => {
    if (lastActive.current === activeCommentId) return;
    lastActive.current = activeCommentId;
    setDocSelection(null);
    window.getSelection()?.removeAllRanges();
  }, [activeCommentId]);

  useEffect(() => {
    backendPromise.then(setBackend);
  }, []);

  useEffect(() => {
    backend
      ?.settings()
      .then(setSettings)
      .catch((e) => notify(`无法读取设置：${errorMessage(e)}`, true));
  }, [backend, notify]);

  useEffect(() => {
    if (!toast) return;
    const t = setTimeout(() => setToast(null), toast.error ? 6000 : 3000);
    return () => clearTimeout(t);
  }, [toast]);

  const navigate = useCallback((next: Page) => {
    setPage(next);
    setVisited((v) => (v.has(next) ? v : new Set(v).add(next)));
  }, []);

  const openDoc = useCallback(
    async (open: () => Promise<OpenedDoc | null>) => {
      if (!(await load(open))) return;
      setActiveCommentId(null);
      setTarget(null);
      setTopBlock(0);
      navigate("doc");
    },
    [load, navigate],
  );

  const openPicker = useCallback(() => backend && openDoc(() => backend.pickAndOpen()), [backend, openDoc]);

  const afterClose = useCallback((closed: boolean) => {
    if (!closed) return;
    setActiveCommentId(null);
    setTarget(null);
    setTopBlock(0);
  }, []);
  const closeDoc = useCallback(() => void close().then(afterClose), [close, afterClose]);
  const saveAndCloseDoc = useCallback(() => void saveAndClose().then(afterClose), [saveAndClose, afterClose]);

  useEffect(() => {
    if (!backend) return;
    void backend.initialFile().then((path) => {
      if (path) void openDoc(() => backend.open(path));
    });
    // Only on startup.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [backend]);

  const pageRef = useRef(page);
  useEffect(() => {
    pageRef.current = page;
  }, [page]);

  useEffect(() => {
    if (!backend) return;
    return backend.onFileDrop((paths) => {
      if (pageRef.current === "kb") {
        setKbDrop({ paths, nonce: Date.now() });
        return;
      }
      const path = paths.find((p) => p.toLowerCase().endsWith(".docx")) ?? paths[0];
      if (path) void openDoc(() => backend.open(path));
    }, setDragging);
  }, [backend, openDoc]);

  // Shortcuts read the latest state through a ref so the listener is added once.
  const shortcuts = useRef({ openPicker, save, saveAs, closeDoc, undo, redo, page, docState, settingsOpen: false });
  useEffect(() => {
    shortcuts.current = { openPicker, save, saveAs, closeDoc, undo, redo, page, docState, settingsOpen: settingsTab !== null };
  });
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.ctrlKey || e.metaKey) || e.altKey) return;
      const s = shortcuts.current;
      const key = e.key.toLowerCase();
      if (s.settingsOpen) return;
      if (key === "o") {
        e.preventDefault();
        void s.openPicker();
      } else if (key === "s") {
        e.preventDefault();
        void (e.shiftKey ? s.saveAs() : s.save());
      } else if (key === "w") {
        e.preventDefault();
        s.closeDoc();
      } else if ((key === "z" || key === "y") && s.page === "doc" && !isTyping(e.target)) {
        e.preventDefault();
        const redoing = key === "y" || e.shiftKey;
        if (redoing && s.docState?.redoLabel) void s.redo();
        else if (!redoing && s.docState?.undoLabel) void s.undo();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const commentsById = useMemo(() => new Map(doc?.summary.comments.map((c) => [c.id, c]) ?? []), [doc]);

  // In the margin the card is already beside its text, so the document only scrolls when that text is off screen.
  const selectComment = useCallback(
    (c: CommentView) => {
      setActiveCommentId(c.id);
      if (c.blockIndex !== null) setTarget({ blockIndex: c.blockIndex, commentId: c.id, nonce: Date.now(), soft: marginLayout });
    },
    [marginLayout],
  );

  // Clicking highlighted text cycles through the comments that cover it.
  const onDocCommentClick = useCallback(
    (ids: string[]) => {
      const threads = ids.filter((id) => !commentsById.get(id)?.parentId);
      if (threads.length === 0) return;
      setActiveCommentId((current) => {
        const at = current ? threads.indexOf(current) : -1;
        return threads[(at + 1) % threads.length];
      });
      setPanelOpen(true);
    },
    [commentsById],
  );

  const onOutlineClick = useCallback(
    (item: OutlineItem) => {
      navigate("doc");
      setTarget({ blockIndex: item.blockIndex, nonce: Date.now() });
    },
    [navigate],
  );

  const onSearch = useCallback(
    (_comment: CommentView, need: string, passage: string, use: (sources: FixSource[]) => void) => setWebSearch({ need, passage, use, nonce: Date.now() }),
    [],
  );
  const closeWebSearch = useCallback(() => setWebSearch(null), []);
  const onSearchCitation = useCallback((query: string) => setWebSearch({ need: query, query, nonce: Date.now() }), []);

  const onOpen = useCallback(() => void openPicker(), [openPicker]);
  const collapseSidebar = useCallback(() => setSidebarOpen(false), []);
  const openSidebar = useCallback(() => setSidebarOpen(true), []);
  const togglePanel = useCallback(() => setPanelOpen((v) => !v), []);
  const openSettings = useCallback(() => setSettingsTab("general"), []);
  const openRoleSettings = useCallback(() => setSettingsTab("roles"), []);
  const closeSettings = useCallback(() => setSettingsTab(null), []);

  const outline = doc?.summary.outline;
  const paragraphCount = doc?.summary.paragraphCount ?? 0;

  // The heading whose section is at the top of the document view (binary search).
  const currentOutline = useMemo(() => {
    const items = outline ?? [];
    let lo = 0;
    let hi = items.length - 1;
    let found = -1;
    while (lo <= hi) {
      const mid = (lo + hi) >> 1;
      if (items[mid].blockIndex <= topBlock) {
        found = mid;
        lo = mid + 1;
      } else hi = mid - 1;
    }
    return found;
  }, [outline, topBlock]);

  // From the current heading to the next heading of the same or a higher level.
  const section = useMemo((): Section | null => {
    if (!outline) return null;
    if (currentOutline < 0) return { start: 0, end: outline[0]?.paragraphIndex ?? paragraphCount, title: "文档开头" };
    const head = outline[currentOutline];
    const next = outline.slice(currentOutline + 1).find((o) => o.level <= head.level);
    return { start: head.paragraphIndex, end: next?.paragraphIndex ?? paragraphCount, title: head.text };
  }, [outline, paragraphCount, currentOutline]);

  // Pre-review results name paragraphs; find the block that holds one (blocks never come after their paragraphs).
  const jumpToParagraph = useCallback(
    async (paragraphIndex: number) => {
      if (!backend || !doc) return;
      const heading = [...doc.summary.outline].reverse().find((o) => o.paragraphIndex <= paragraphIndex);
      const from = heading?.blockIndex ?? 0;
      try {
        const blocks = await backend.blocks(doc.docId, from, Math.min(doc.summary.blockCount, paragraphIndex + 1));
        const block = blocks.find((b) =>
          b.kind === "paragraph"
            ? b.paragraph.index === paragraphIndex
            : b.rows.some((row) => row.some((cell) => cell.paragraphs.some((p) => p.index === paragraphIndex))),
        );
        navigate("doc");
        setTarget({ blockIndex: block?.index ?? from, paragraphIndex, nonce: Date.now() });
      } catch (e) {
        notify(`无法定位段落：${errorMessage(e)}`, true);
      }
    },
    [backend, doc, navigate, notify],
  );

  const onPage = page === "doc";

  return (
    <div className="app">
      <aside className={`sidebar${sidebarOpen ? "" : " closed"}`}>
        <Sidebar
          doc={doc}
          currentOutline={currentOutline}
          page={page}
          onNavigate={navigate}
          onOpen={onOpen}
          onOpenSettings={openSettings}
          onOutlineClick={onOutlineClick}
          onCollapse={collapseSidebar}
        />
      </aside>

      <main className="main">
        <TopBar
          doc={doc}
          docState={docState}
          pageTitle={PAGE_TITLE[page]}
          sidebarOpen={sidebarOpen}
          panelOpen={panelOpen}
          onOpenSidebar={openSidebar}
          onTogglePanel={togglePanel}
          onUndo={undo}
          onRedo={redo}
          onSave={save}
          onSaveAs={saveAs}
          onSaveAndClose={saveAndCloseDoc}
          onClose={closeDoc}
        />

        <div className="workspace">
          <div className={`doc-page${onPage ? "" : " hidden"}`} inert={!onPage}>
            {doc && backend ? (
              <>
                <DocumentView
                  doc={doc}
                  version={version}
                  backend={backend}
                  activeCommentId={activeCommentId}
                  target={target}
                  onCommentClick={onDocCommentClick}
                  onTopBlockChange={setTopBlock}
                  onSelectionChange={onSelectionChange}
                  margin={marginLayout && panelOpen}
                  onMarginHost={setMarginHost}
                />
                <aside
                  className={`panel${panelOpen ? "" : " closed"}${marginLayout ? " margin-layout" : ""}${panelWidth.resizing ? " resizing" : ""}`}
                  style={panelWidth.width === null ? undefined : ({ "--panel-w": `${panelWidth.width}px` } as CSSProperties)}
                >
                  <div
                    className="panel-resizer"
                    role="separator"
                    aria-orientation="vertical"
                    title="拖动调整批注栏宽度，双击恢复默认"
                    onPointerDown={panelWidth.onPointerDown}
                    onDoubleClick={panelWidth.reset}
                  />
                  <CommentsPanel
                    key={doc.docId}
                    backend={backend}
                    docId={doc.docId}
                    comments={doc.summary.comments}
                    authors={authors}
                    activeId={activeCommentId}
                    selection={docSelection}
                    needsModel={settings !== null && !settings.roles.chat.model}
                    onSelect={selectComment}
                    edit={edit}
                    onAuthors={setAuthors}
                    onOpenSettings={openRoleSettings}
                    onSearch={onSearch}
                    notify={notify}
                    layout={commentLayout}
                    onLayout={setCommentLayout}
                    marginHost={marginLayout ? marginHost : null}
                  />
                </aside>
              </>
            ) : (
              <div className="welcome">
                <div className="welcome-card">
                  <img src="/logo.svg" alt="" />
                  <h1>让每一轮评审都更快通过</h1>
                  <p>打开一份带批注的 Word 报告，按批注逐条定位、查看和处理。</p>
                  {loading ? (
                    <div className="loading" style={{ justifyContent: "center" }}>
                      <div className="spinner" /> 正在解析文档…
                    </div>
                  ) : (
                    <button className="btn primary" onClick={() => void openPicker()} disabled={!backend}>
                      <FolderOpen size={16} /> 打开 .docx 文档
                    </button>
                  )}
                  <div className="hint">也可以把文件拖到窗口中 · 文档只在本机处理</div>
                </div>
              </div>
            )}
          </div>
          {backend && visited.has("proofread") && (
            <div className={`page-layer${page === "proofread" ? "" : " hidden"}`} inert={page !== "proofread"}>
              <ProofreadPage
                backend={backend}
                doc={doc}
                section={section}
                settings={settings}
                edit={edit}
                onJump={jumpToParagraph}
                onSearch={onSearchCitation}
                onOpenSettings={openRoleSettings}
                notify={notify}
              />
            </div>
          )}
          {backend && visited.has("kb") && (
            <div className={`page-layer${page === "kb" ? "" : " hidden"}`} inert={page !== "kb"}>
              <KnowledgeBasePage backend={backend} active={page === "kb"} settings={settings} dropped={kbDrop} notify={notify} />
            </div>
          )}
          {backend && visited.has("reviewers") && (
            <div className={`page-layer${page === "reviewers" ? "" : " hidden"}`} inert={page !== "reviewers"}>
              <ReviewersPage
                backend={backend}
                active={page === "reviewers"}
                docId={doc?.docId ?? null}
                section={section}
                onJump={jumpToParagraph}
                notify={notify}
              />
            </div>
          )}
        </div>

        {dragging && <div className="drop-overlay">{page === "kb" ? "松开以导入知识库" : "松开以打开文档"}</div>}
        {toast && <div className={`toast${toast.error ? " error" : ""}`}>{toast.text}</div>}
      </main>

      {backend && webSearch && <WebSearchDialog key={webSearch.nonce} backend={backend} request={webSearch} onClose={closeWebSearch} notify={notify} />}
      {backend && settingsTab && (
        <SettingsDialog
          backend={backend}
          initialTab={settingsTab}
          theme={theme}
          onTheme={setTheme}
          layout={commentLayout}
          onLayout={setCommentLayout}
          onClose={closeSettings}
          onSaved={setSettings}
        />
      )}
    </div>
  );
}
