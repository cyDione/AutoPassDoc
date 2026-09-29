import { memo, useState } from "react";
import { AlertTriangle, ChevronDown, ChevronRight, FileText, RefreshCw } from "lucide-react";
import type { Citation, FixProposal, Judgement } from "../types";
import { percent } from "../util";
import { DiffView } from "./DiffView";
import { Floating } from "./Floating";

interface Props {
  proposal: FixProposal;
  busy: boolean;
  error: string | undefined;
  onApply: (edited: string[] | null, force: boolean) => void;
  onRegenerate: () => void;
  onReject: () => void;
  onOpenCitation: (path: string) => void;
}

/** Why a judged proposal may not be applied with one click. */
function failReasons(judge: Judgement): string[] {
  const reasons: string[] = [];
  if (judge.confidence < judge.threshold) reasons.push(`置信度 ${percent(judge.confidence)} 低于阈值 ${percent(judge.threshold)}`);
  const hard = judge.items.filter((i) => i.hard && !i.passed).map((i) => `“${i.label}”`);
  if (hard.length) reasons.push(`${hard.join("、")}未达标`);
  return reasons;
}

function ConfidencePill({ proposal }: { proposal: FixProposal }) {
  const { judge } = proposal;
  if (!judge) {
    return (
      <span className="pill neutral" title={proposal.judgeError ?? "没有配置决策模型"}>
        未评判
      </span>
    );
  }
  if (judge.passed) return <span className="pill positive">置信度 {percent(judge.confidence)}</span>;
  const below = judge.confidence < judge.threshold;
  return (
    <span className="pill warn" title={failReasons(judge).join("；")}>
      置信度 {percent(judge.confidence)} · {below ? `低于阈值 ${percent(judge.threshold)}` : "必须达标项未通过"}
    </span>
  );
}

function JudgeDetails({ proposal }: { proposal: FixProposal }) {
  const judge = proposal.judge!;
  const seconds = (proposal.elapsedMs / 1000).toFixed(1);
  const { passages, examples, profile } = proposal.context;
  const context = [passages && `${passages} 段资料`, examples && `${examples} 条历史案例`, profile && "审稿人画像"].filter(Boolean);
  return (
    <div className="judge-items">
      {judge.items.map((item) => (
        <div key={item.key} className={`judge-item${item.passed ? "" : " failed"}`}>
          <span className="label">{item.label}</span>
          <span className="bar" title={item.hard ? `须达到 ${percent(judge.threshold)}` : undefined}>
            <span className="fill" style={{ width: percent(item.value) }} />
            {item.hard && <span className="tick" style={{ left: percent(judge.threshold) }} />}
          </span>
          <span className="pct">{percent(item.value)}</span>
          {item.hard && !item.passed && <span className="must">必须达标</span>}
        </div>
      ))}
      <div className="judge-foot">
        评判：{judge.backend === "jev" ? "Jev 决策接口" : "大语言模型"} · {judge.model}
        <br />
        生成：{proposal.model} · {seconds} 秒{context.length > 0 && ` · 参考 ${context.join("、")}`}
      </div>
    </div>
  );
}

function CitationChips({ citations, onOpen }: { citations: Citation[]; onOpen: (path: string) => void }) {
  const [hover, setHover] = useState<{ anchor: HTMLElement; citation: Citation } | null>(null);
  return (
    <div className="cite-chips">
      {citations.map((c) => (
        <button
          key={`${c.n}-${c.chunkId}`}
          type="button"
          className="cite-chip"
          onClick={() => onOpen(c.storedPath)}
          onMouseEnter={(e) => setHover({ anchor: e.currentTarget, citation: c })}
          onMouseLeave={() => setHover(null)}
          onFocus={(e) => setHover({ anchor: e.currentTarget, citation: c })}
          onBlur={() => setHover(null)}
        >
          <span className="n">[{c.n}]</span>
          <span className="text">
            {c.fileName}
            {c.headingPath.length > 0 && ` · ${c.headingPath.join(" › ")}`}
          </span>
        </button>
      ))}
      {hover && (
        <Floating anchor={hover.anchor} width={320} className="hover-card">
          <div className="hover-title">
            <FileText size={13} /> {hover.citation.title}
          </div>
          {hover.citation.headingPath.length > 0 && <div className="hover-path">{hover.citation.headingPath.join(" › ")}</div>}
          <div className="hover-text">{hover.citation.text}</div>
          <div className="hover-hint">点击用默认程序打开</div>
        </Floating>
      )}
    </div>
  );
}

/** An AI fix inside a comment card: diff, judgement, sources and actions. */
export const FixResult = memo(function FixResult({ proposal, busy, error, onApply, onRegenerate, onReject, onOpenCitation }: Props) {
  const [details, setDetails] = useState(false);
  const [editing, setEditing] = useState<string[] | null>(null);
  const [confirming, setConfirming] = useState(false);
  const { judge } = proposal;
  const blocked = judge !== null && !judge.passed;

  return (
    <div className="fix-result" onClick={(e) => e.stopPropagation()}>
      <div className="fix-head">
        <ConfidencePill proposal={proposal} />
        {judge?.category && <span className="tag">{judge.category}</span>}
        {proposal.reviewer && <span className="tag subtle">{proposal.reviewer}</span>}
        <span className="spacer" />
        {judge && (
          <button type="button" className="link-btn" onClick={() => setDetails((v) => !v)}>
            评判详情 {details ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
          </button>
        )}
      </div>
      {details && judge && <JudgeDetails proposal={proposal} />}

      {editing ? (
        <div className="fix-edit">
          {editing.map((text, i) => (
            <textarea
              key={proposal.paragraphs[i].index}
              className="textarea"
              value={text}
              rows={Math.min(12, Math.max(3, Math.ceil(text.length / 22)))}
              onChange={(e) => setEditing(editing.map((t, j) => (j === i ? e.target.value : t)))}
            />
          ))}
        </div>
      ) : (
        proposal.paragraphs.map((p) => <DiffView key={p.index} diff={p.diff} />)
      )}

      {proposal.explanation && <p className="fix-explanation">{proposal.explanation}</p>}
      {proposal.citations.length > 0 && <CitationChips citations={proposal.citations} onOpen={onOpenCitation} />}
      {proposal.warnings.map((w) => (
        <div key={w} className="fix-note warn">
          <AlertTriangle size={13} /> {w}
        </div>
      ))}
      {error && <div className="fix-note error">{error}</div>}

      {confirming && judge && (
        <div className="fix-confirm">
          <div>未通过评判：{failReasons(judge).join("；")}。确定仍然应用？</div>
          <div className="row">
            <button
              type="button"
              className="btn sm danger"
              disabled={busy}
              onClick={() => {
                setConfirming(false);
                onApply(null, true);
              }}
            >
              确定应用
            </button>
            <button type="button" className="btn sm" onClick={() => setConfirming(false)}>
              取消
            </button>
          </div>
        </div>
      )}

      <div className="fix-actions">
        {editing ? (
          <>
            <button type="button" className="btn sm primary" disabled={busy} onClick={() => onApply(editing, true)}>
              {busy && <span className="spinner sm" />}应用修改
            </button>
            <button type="button" className="btn sm" disabled={busy} onClick={() => setEditing(null)}>
              取消
            </button>
          </>
        ) : (
          <>
            <button
              type="button"
              className={`btn sm${blocked ? "" : " primary"}`}
              disabled={busy || confirming}
              onClick={() => (blocked ? setConfirming(true) : onApply(null, false))}
            >
              {busy && <span className="spinner sm" />}
              {blocked ? "仍然应用" : "应用"}
            </button>
            <button type="button" className="btn sm" disabled={busy} onClick={() => setEditing(proposal.paragraphs.map((p) => p.new))}>
              编辑
            </button>
            <button type="button" className="btn sm" disabled={busy} onClick={onRegenerate} title="重新生成">
              <RefreshCw size={12} /> 重新生成
            </button>
            <button type="button" className="btn sm ghost" disabled={busy} onClick={onReject}>
              忽略
            </button>
            {!judge && <span className="fix-unjudged">未经评判</span>}
          </>
        )}
      </div>
    </div>
  );
});
