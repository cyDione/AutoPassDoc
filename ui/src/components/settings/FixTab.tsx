import { useState } from "react";
import type { Settings } from "../../types";
import { percent } from "../../util";

type Fix = Settings["fix"];

interface Props {
  fix: Fix;
  onChange: (fix: Fix) => void;
}

interface IntProps {
  value: number;
  min: number;
  max: number;
  disabled?: boolean;
  onChange: (value: number) => void;
}

/** An integer input that may be empty while typing; it commits values in range and snaps back on blur. */
function IntField({ value, min, max, disabled, onChange }: IntProps) {
  const [text, setText] = useState(String(value));
  const [prev, setPrev] = useState(value);
  if (value !== prev) {
    setPrev(value);
    setText(String(value));
  }
  return (
    <input
      className="input tiny"
      type="number"
      min={min}
      max={max}
      value={text}
      disabled={disabled}
      onChange={(e) => {
        setText(e.target.value);
        const n = Number(e.target.value);
        if (e.target.value.trim() !== "" && Number.isInteger(n) && n >= min && n <= max) onChange(n);
      }}
      onBlur={() => setText(String(value))}
    />
  );
}

export function FixTab({ fix, onChange }: Props) {
  const set = <K extends keyof Fix>(key: K, value: Fix[K]) => onChange({ ...fix, [key]: value });

  return (
    <div className="settings-section">
      <div className="section-head">
        <h3>AI 修复</h3>
      </div>

      <div className="setting-row">
        <div className="setting-text">
          <div className="setting-label">置信度阈值</div>
          <div className="setting-hint">综合置信度达到阈值、且必须达标的各项都通过时，才能一键应用。可在审稿人页为个别专家单独设置。</div>
        </div>
        <div className="slider">
          <input
            type="range"
            min={0.5}
            max={0.95}
            step={0.05}
            value={fix.threshold}
            onChange={(e) => set("threshold", Number(e.target.value))}
          />
          <span className="slider-value">{percent(fix.threshold)}</span>
        </div>
      </div>

      <div className="setting-row">
        <div className="setting-text">
          <div className="setting-label">写回方式</div>
          <div className="setting-hint">修订模式会在 Word 中显示删改痕迹，方便再发给专家确认。</div>
        </div>
        <div className="radio-group vertical">
          <label className="radio">
            <input type="radio" checked={fix.editMode === "tracked"} onChange={() => set("editMode", "tracked")} />
            修订模式（推荐）
          </label>
          <label className="radio">
            <input type="radio" checked={fix.editMode === "direct"} onChange={() => set("editMode", "direct")} />
            直接替换
          </label>
        </div>
      </div>

      <div className="setting-row">
        <div className="setting-text">
          <div className="setting-label">修订作者</div>
          <div className="setting-hint">Word 中修订和回复显示的作者名。</div>
        </div>
        <input className="input narrow" value={fix.author} onChange={(e) => set("author", e.target.value)} />
      </div>

      <div className="setting-row">
        <label className="check">
          <input type="checkbox" checked={fix.resolveOnApply} onChange={(e) => set("resolveOnApply", e.target.checked)} />
          应用后标记批注已解决
        </label>
      </div>

      <div className="setting-row stacked">
        <label className="check">
          <input type="checkbox" checked={fix.replyOnApply} onChange={(e) => set("replyOnApply", e.target.checked)} />
          应用后回复批注
        </label>
        <input
          className="input"
          value={fix.replyText}
          disabled={!fix.replyOnApply}
          placeholder="回复内容，例如：已根据该意见修改。"
          onChange={(e) => set("replyText", e.target.value)}
        />
      </div>

      <div className="setting-row">
        <label className="check">
          <input type="checkbox" checked={fix.useKb} onChange={(e) => set("useKb", e.target.checked)} />
          使用知识库
        </label>
        <label className="inline-field">
          引用片段数
          <IntField value={fix.kbPassages} min={1} max={20} disabled={!fix.useKb} onChange={(v) => set("kbPassages", v)} />
        </label>
      </div>

      <div className="setting-row">
        <div className="setting-text">
          <div className="setting-label">批量并发数</div>
          <div className="setting-hint">批量修复时同时发出的请求数，受服务商限流影响。</div>
        </div>
        <IntField value={fix.concurrency} min={1} max={16} onChange={(v) => set("concurrency", v)} />
      </div>

      <div className="setting-row">
        <div className="setting-text">
          <div className="setting-label">画像更新频率（每 N 条案例）</div>
          <div className="setting-hint">某位审稿人每新增 N 条已处理的批注，就重新提炼一次画像。</div>
        </div>
        <IntField value={fix.profileEvery} min={1} max={100} onChange={(v) => set("profileEvery", v)} />
      </div>
    </div>
  );
}
