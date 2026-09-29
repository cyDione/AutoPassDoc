import { memo } from "react";
import { MessageSquareText, PanelLeftOpen, Redo2, Save, Undo2 } from "lucide-react";
import type { DocState, OpenedDoc } from "../types";
import { formatChars } from "../util";

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
}: Props) {
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
          <button className="btn" onClick={onSave} title={docState?.savedPath ? `保存到 ${docState.savedPath}（Ctrl+S）` : "保存（Ctrl+S）"}>
            <Save size={15} /> 保存
          </button>
          <button className="btn" onClick={onSaveAs} title="另存为（Ctrl+Shift+S）">
            另存为
          </button>
          <button className={`icon-btn${panelOpen ? " on" : ""}`} title={panelOpen ? "隐藏批注" : "显示批注"} onClick={onTogglePanel}>
            <MessageSquareText size={17} />
          </button>
        </>
      )}
    </header>
  );
});
