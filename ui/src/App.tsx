import { useCallback, useEffect, useMemo, useState } from "react";
import { FolderOpen, MessageSquareText, PanelLeftOpen, Save } from "lucide-react";
import { backend as backendPromise, type Backend } from "./api";
import { CommentsPanel } from "./components/CommentsPanel";
import { DocumentView, type ScrollTarget } from "./components/DocumentView";
import { Sidebar, type Theme } from "./components/Sidebar";
import type { CommentView, OpenedDoc, OutlineItem } from "./types";
import { formatChars } from "./util";

function readTheme(): Theme {
  try {
    const t = localStorage.getItem("theme");
    if (t === "light" || t === "dark" || t === "word" || t === "system") return t;
  } catch {
    /* storage unavailable */
  }
  return "system";
}

function useTheme(): [Theme, (t: Theme) => void] {
  const [theme, setTheme] = useState<Theme>(readTheme);
  useEffect(() => {
    const media = window.matchMedia("(prefers-color-scheme: dark)");
    const apply = () => {
      const resolved = theme === "system" ? (media.matches ? "dark" : "light") : theme;
      document.documentElement.dataset.theme = resolved;
    };
    apply();
    try {
      localStorage.setItem("theme", theme);
    } catch {
      /* storage unavailable */
    }
    media.addEventListener("change", apply);
    return () => media.removeEventListener("change", apply);
  }, [theme]);
  return [theme, setTheme];
}

interface Toast {
  text: string;
  error?: boolean;
}

export default function App() {
  const [backend, setBackend] = useState<Backend | null>(null);
  const [doc, setDoc] = useState<OpenedDoc | null>(null);
  const [loading, setLoading] = useState(false);
  const [toast, setToast] = useState<Toast | null>(null);
  const [theme, setTheme] = useTheme();
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const [panelOpen, setPanelOpen] = useState(true);
  const [activeCommentId, setActiveCommentId] = useState<string | null>(null);
  const [target, setTarget] = useState<ScrollTarget | null>(null);
  const [topBlock, setTopBlock] = useState(0);
  const [dragging, setDragging] = useState(false);

  useEffect(() => {
    backendPromise.then(setBackend);
  }, []);

  useEffect(() => {
    if (!toast) return;
    const t = setTimeout(() => setToast(null), toast.error ? 6000 : 3000);
    return () => clearTimeout(t);
  }, [toast]);

  const load = useCallback(
    async (open: () => Promise<OpenedDoc | null>) => {
      setLoading(true);
      try {
        const next = await open();
        if (!next) return;
        if (doc && backend) void backend.close(doc.docId);
        setDoc(next);
        setActiveCommentId(null);
        setTarget(null);
        setTopBlock(0);
      } catch (e) {
        setToast({ text: `无法打开文档：${String(e)}`, error: true });
      } finally {
        setLoading(false);
      }
    },
    [backend, doc],
  );

  const openPicker = useCallback(() => backend && load(() => backend.pickAndOpen()), [backend, load]);

  const saveAs = useCallback(async () => {
    if (!backend || !doc) return;
    try {
      const path = await backend.saveAs(doc.docId);
      if (path) setToast({ text: `已另存为 ${path}` });
    } catch (e) {
      setToast({ text: `保存失败：${String(e)}`, error: true });
    }
  }, [backend, doc]);

  useEffect(() => {
    if (!backend) return;
    void backend.initialFile().then((path) => {
      if (path) void load(() => backend.open(path));
    });
    // Only on startup.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [backend]);

  useEffect(() => {
    if (!backend) return;
    return backend.onFileDrop((paths) => {
      const path = paths.find((p) => p.toLowerCase().endsWith(".docx")) ?? paths[0];
      if (path) void load(() => backend.open(path));
    }, setDragging);
  }, [backend, load]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.ctrlKey || e.metaKey)) return;
      if (e.key.toLowerCase() === "o") {
        e.preventDefault();
        void openPicker();
      } else if (e.key.toLowerCase() === "s" && e.shiftKey) {
        e.preventDefault();
        void saveAs();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [openPicker, saveAs]);

  const commentsById = useMemo(() => new Map(doc?.summary.comments.map((c) => [c.id, c]) ?? []), [doc]);

  const selectComment = useCallback((c: CommentView) => {
    setActiveCommentId(c.id);
    if (c.blockIndex !== null) setTarget({ blockIndex: c.blockIndex, commentId: c.id, nonce: Date.now() });
  }, []);

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

  const onOutlineClick = useCallback((item: OutlineItem) => {
    setTarget({ blockIndex: item.blockIndex, nonce: Date.now() });
  }, []);

  const onOpen = useCallback(() => void openPicker(), [openPicker]);
  const collapseSidebar = useCallback(() => setSidebarOpen(false), []);

  // The heading whose section is at the top of the document view (binary search).
  const currentOutline = useMemo(() => {
    const outline = doc?.summary.outline ?? [];
    let lo = 0;
    let hi = outline.length - 1;
    let found = -1;
    while (lo <= hi) {
      const mid = (lo + hi) >> 1;
      if (outline[mid].blockIndex <= topBlock) {
        found = mid;
        lo = mid + 1;
      } else hi = mid - 1;
    }
    return found;
  }, [doc, topBlock]);

  const topLevelComments = doc?.summary.comments.filter((c) => !c.parentId) ?? [];
  const openComments = topLevelComments.filter((c) => !c.done).length;

  return (
    <div className="app">
      <aside className={`sidebar${sidebarOpen ? "" : " closed"}`}>
        <Sidebar
          doc={doc}
          currentOutline={currentOutline}
          theme={theme}
          onThemeChange={setTheme}
          onOpen={onOpen}
          onOutlineClick={onOutlineClick}
          onCollapse={collapseSidebar}
        />
      </aside>

      <main className="main">
        <header className="topbar">
          {!sidebarOpen && (
            <button className="icon-btn" title="展开侧边栏" onClick={() => setSidebarOpen(true)}>
              <PanelLeftOpen size={17} />
            </button>
          )}
          {doc ? (
            <div className="title">
              <span className="name">{doc.fileName}</span>
              <span className="meta">
                {formatChars(doc.summary.charCount)} · {topLevelComments.length} 条批注，{openComments} 条未解决
              </span>
            </div>
          ) : (
            <div className="title">
              <span className="name">AutoPassDoc</span>
            </div>
          )}
          <span className="spacer" />
          {doc && (
            <>
              <button className="btn" onClick={() => void saveAs()} title="另存为（Ctrl+Shift+S）">
                <Save size={15} /> 另存为
              </button>
              <button
                className={`icon-btn${panelOpen ? " on" : ""}`}
                title={panelOpen ? "隐藏批注" : "显示批注"}
                onClick={() => setPanelOpen((v) => !v)}
              >
                <MessageSquareText size={17} />
              </button>
            </>
          )}
        </header>

        <div className="workspace">
          {doc && backend ? (
            <>
              <DocumentView
                doc={doc}
                backend={backend}
                activeCommentId={activeCommentId}
                target={target}
                onCommentClick={onDocCommentClick}
                onTopBlockChange={setTopBlock}
              />
              <aside className={`panel${panelOpen ? "" : " closed"}`}>
                <CommentsPanel comments={doc.summary.comments} activeId={activeCommentId} onSelect={selectComment} />
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

        {dragging && <div className="drop-overlay">松开以打开文档</div>}
        {toast && <div className={`toast${toast.error ? " error" : ""}`}>{toast.text}</div>}
      </main>
    </div>
  );
}
