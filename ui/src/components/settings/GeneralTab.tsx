import { Columns2, LayoutList, Monitor, Moon, Sun, Type } from "lucide-react";
import type { CommentLayout } from "../../hooks/useCommentLayout";
import type { Theme } from "../../hooks/useTheme";

interface Props {
  theme: Theme;
  onTheme: (theme: Theme) => void;
  layout: CommentLayout;
  onLayout: (layout: CommentLayout) => void;
}

const THEMES = [
  ["light", Sun, "浅色"],
  ["dark", Moon, "深色"],
  ["word", Type, "Word 配色"],
  ["system", Monitor, "跟随系统"],
] as const;

const LAYOUTS = [
  ["dense", LayoutList, "密集列表", "批注集中列在右侧栏，只展开选中的一条，适合逐条处理"],
  ["margin", Columns2, "经典 Word", "批注显示在页边、与所批注的文字同高，随正文滚动"],
] as const;

/** Interface preferences; they apply at once and are kept on this computer. */
export function GeneralTab({ theme, onTheme, layout, onLayout }: Props) {
  return (
    <div className="settings-section">
      <div className="section-head">
        <h3>通用</h3>
      </div>
      <div className="setting-row stacked">
        <div className="setting-text">
          <div className="setting-label">外观</div>
          <div className="setting-hint">点选即生效，不需要保存。</div>
        </div>
        <div className="choice-cards">
          {THEMES.map(([key, Icon, label]) => (
            <button key={key} type="button" className={`choice-card${theme === key ? " on" : ""}`} onClick={() => onTheme(key)}>
              <Icon size={18} strokeWidth={1.6} />
              <span>{label}</span>
            </button>
          ))}
        </div>
      </div>
      <div className="setting-row stacked">
        <div className="setting-text">
          <div className="setting-label">批注排版</div>
          <div className="setting-hint">也可以在批注栏标题旁切换。</div>
        </div>
        <div className="choice-cards wide">
          {LAYOUTS.map(([key, Icon, label, hint]) => (
            <button key={key} type="button" className={`choice-card${layout === key ? " on" : ""}`} onClick={() => onLayout(key)}>
              <Icon size={18} strokeWidth={1.6} />
              <span>{label}</span>
              <small>{hint}</small>
            </button>
          ))}
        </div>
      </div>
    </div>
  );
}
