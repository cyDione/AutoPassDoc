import { memo, useRef, useState } from "react";
import { ChevronDown, MessageSquareText, PanelLeftOpen, Redo2, Save, Undo2, X } from "lucide-react";
import type { DocState, OpenedDoc } from "../types";
import { formatChars } from "../util";
import { Floating } from "./Floating";

interface Props {
  doc: OpenedDoc | null;
  docState: DocState | null;
  /** Title of a non-document page; the document controls are hidden there. */
  pageTitle: string | null;
  sidebarOpen: boolean;
  panelOpen: boolean;
  onOpenSidebar: () => void;
  onTogglePanel: () => void;
  onUndo: () => void;
  onRedo: () => void;
  onSave: () => void;
  onSaveAs: () => void;
  onSaveAndClose: () => void;
  onClose: () => void;
}

export const TopBar = memo(function TopBar({
  doc,
  docState,
  pageTitle,
  sidebarOpen,
  panelOpen,
  onOpenSidebar,
  onTogglePanel,
  onUndo,
  onRedo,
  onSave,
  onSaveAs,
  onSaveAndClose,
  onClose,
}: Props) {
  const saveRef = useRef<HTMLDivElement>(null);
  const [menuOpen, setMenuOpen] = useState(false);
  const pick = (run: () => void) => () => {
    setMenuOpen(false);
    run();
  };
  const threads = doc?.summary.comments.filter((c) => !c.parentId) ?? [];
  const open = threads.filter((c) => !c.done).length;
  const showDoc = doc && !pageTitle;

  return (
    <header className="topbar">
      {!sidebarOpen && (
        <button className="icon-btn" title="展开侧边栏" onClick={onOpenSidebar}>
          <PanelLeftOpen size={17} />
        </button>
      )}
      {showDoc ? (
        <div className="title">
          <span className="name-row">
            <span className="name">{doc.fileName}</span>
            {docState?.dirty && (
              <span className="unsaved" title="有未保存的修改">
                <i /> 未保存
              </span>
            )}
          </span>
          <span className="meta">
            {formatChars(doc.summary.charCount)} · {threads.length} 条批注，{open} 条未解决
          </span>
        </div>
      ) : (
        <div className="title">
          <span className="name">{pageTitle ?? "AutoPassDoc"}</span>
        </div>
      )}
      <span className="spacer" />
      {showDoc && (
        <>
          <button
            className="icon-btn"
            title={docState?.undoLabel ? `撤销：${docState.undoLabel}` : "没有可撤销的操作"}
            disabled={!docState?.undoLabel}
            onClick={onUndo}
          >
            <Undo2 size={17} />
          </button>
          <button
            className="icon-btn"
            title={docState?.redoLabel ? `重做：${docState.redoLabel}` : "没有可重做的操作"}
            disabled={!docState?.redoLabel}
            onClick={onRedo}
          >
            <Redo2 size={17} />
          </button>
          <span className="topbar-sep" />
          <div className="split-btn" ref={saveRef}>
            <button className="btn" onClick={onSave} title={docState?.savedPath ? `保存到 ${docState.savedPath}（Ctrl+S）` : "保存（Ctrl+S）"}>
              <Save size={15} /> 保存
            </button>
            <button
              className={`btn split-arrow${menuOpen ? " on" : ""}`}
              title="更多保存方式"
              aria-haspopup="menu"
              aria-expanded={menuOpen}
              onClick={() => setMenuOpen((v) => !v)}
            >
              <ChevronDown size={14} />
            </button>
          </div>
          {menuOpen && saveRef.current && (
            <Floating anchor={saveRef.current} onClose={() => setMenuOpen(false)} width={200} align="end" className="dropdown menu">
              <div role="menu">
                <button role="menuitem" className="dropdown-item" onClick={pick(onSaveAs)}>
                  另存为…<span className="shortcut">Ctrl+Shift+S</span>
                </button>
                <button role="menuitem" className="dropdown-item" onClick={pick(onSaveAndClose)}>
                  保存并关闭
                </button>
              </div>
            </Floating>
          )}
          <button className={`icon-btn${panelOpen ? " on" : ""}`} title={panelOpen ? "隐藏批注" : "显示批注"} onClick={onTogglePanel}>
            <MessageSquareText size={17} />
          </button>
          <button className="icon-btn" title="关闭文档（Ctrl+W）" onClick={onClose}>
            <X size={17} />
          </button>
        </>
      )}
    </header>
  );
});
