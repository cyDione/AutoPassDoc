import { memo, useEffect, useRef } from "react";
import { FileSearch, FileText, FolderOpen, Library, PanelLeftClose, Settings, Users } from "lucide-react";
import type { OpenedDoc, OutlineItem } from "../types";

export type Page = "doc" | "proofread" | "kb" | "reviewers";

const PAGES = [
  ["doc", FileText, "文档"],
  ["proofread", FileSearch, "文档校对"],
  ["kb", Library, "知识库"],
  ["reviewers", Users, "审稿人"],
] as const;

interface Props {
  doc: OpenedDoc | null;
  /** Index into the outline of the section at the top of the document view. */
  currentOutline: number;
  page: Page;
  onNavigate: (page: Page) => void;
  onOpen: () => void;
  onOpenSettings: () => void;
  onOutlineClick: (item: OutlineItem) => void;
  onCollapse: () => void;
}

export const Sidebar = memo(function Sidebar({
  doc,
  currentOutline: current,
  page,
  onNavigate,
  onOpen,
  onOpenSettings,
  onOutlineClick,
  onCollapse,
}: Props) {
  const outline = doc?.summary.outline ?? [];
  const listRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    listRef.current?.querySelector(".outline-item.current")?.scrollIntoView({ block: "nearest" });
  }, [current]);

  return (
    <>
      <div className="brand">
        <img src="/logo.svg" alt="" />
        <span>AutoPassDoc</span>
        <span className="spacer" />
        <button className="icon-btn" title="收起侧边栏" onClick={onCollapse}>
          <PanelLeftClose size={17} />
        </button>
      </div>
      <button className="side-action" onClick={onOpen}>
        <FolderOpen size={16} /> 打开文档
        <span className="shortcut">Ctrl+O</span>
      </button>
      <nav className="side-nav">
        {PAGES.map(([key, Icon, label]) => (
          <button key={key} className={`side-action${page === key ? " on" : ""}`} onClick={() => onNavigate(key)}>
            <Icon size={16} /> {label}
          </button>
        ))}
      </nav>

      {doc && (
        <>
          <div className="side-section">大纲</div>
          <div className="outline scroll" ref={listRef}>
            {outline.length === 0 && <div className="side-section">文档中没有标题</div>}
            {outline.map((item, i) => (
              <button
                key={item.blockIndex}
                className={`outline-item l${item.level}${i === current ? " current" : ""}`}
                style={{ paddingLeft: 10 + item.level * 14 }}
                title={item.text}
                onClick={() => onOutlineClick(item)}
              >
                {item.text}
              </button>
            ))}
          </div>
        </>
      )}
      {!doc && <div style={{ flex: 1 }} />}

      <div className="sidebar-footer">
        <button className="side-action" title="设置（外观、模型、联网搜索、数据备份、关于）" onClick={onOpenSettings}>
          <Settings size={16} /> 设置
        </button>
      </div>
    </>
  );
});
