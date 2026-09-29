import { memo, useEffect, useRef } from "react";
import { FileText, FolderOpen, Library, Monitor, Moon, PanelLeftClose, Settings, Sun, Type, Users } from "lucide-react";
import type { Theme } from "../hooks/useTheme";
import type { OpenedDoc, OutlineItem } from "../types";
import { formatChars } from "../util";

export type Page = "doc" | "kb" | "reviewers";

const PAGES = [
  ["doc", FileText, "文档"],
  ["kb", Library, "知识库"],
  ["reviewers", Users, "审稿人"],
] as const;

interface Props {
  doc: OpenedDoc | null;
  /** Index into the outline of the section at the top of the document view. */
  currentOutline: number;
  page: Page;
  theme: Theme;
  onNavigate: (page: Page) => void;
  onThemeChange: (theme: Theme) => void;
  onOpen: () => void;
  onOpenSettings: () => void;
  onOutlineClick: (item: OutlineItem) => void;
  onCollapse: () => void;
}

export const Sidebar = memo(function Sidebar({
  doc,
  currentOutline: current,
  page,
  theme,
  onNavigate,
  onThemeChange,
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
          <div className="side-section">当前文档</div>
          <div className="doc-item" title={doc.path}>
            <div className="name">
              <FileText size={13} style={{ verticalAlign: -2, marginRight: 6 }} />
              {doc.fileName}
            </div>
            <div className="meta">
              {formatChars(doc.summary.charCount)} · {doc.summary.comments.filter((c) => !c.parentId).length} 条批注
            </div>
          </div>
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
        <span className="label">外观</span>
        {(
          [
            ["light", Sun, "浅色"],
            ["dark", Moon, "深色"],
            ["word", Type, "Word 配色"],
            ["system", Monitor, "跟随系统"],
          ] as const
        ).map(([key, Icon, label]) => (
          <button key={key} className={`icon-btn${theme === key ? " on" : ""}`} title={label} onClick={() => onThemeChange(key)}>
            <Icon size={15} />
          </button>
        ))}
        <span className="footer-sep" />
        <button className="icon-btn" title="设置" onClick={onOpenSettings}>
          <Settings size={15} />
        </button>
      </div>
    </>
  );
});
