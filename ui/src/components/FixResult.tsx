import { memo, useState } from "react";
import { AlertTriangle, ChevronDown, ChevronRight, FileText, Globe, PenLine, RefreshCw, Sparkles } from "lucide-react";
import { PLACEHOLDER, type Citation, type FixMode, type FixProposal, type Judgement } from "../types";
import { hasPlaceholder, percent } from "../util";
import { DiffView } from "./DiffView";
import { Floating } from "./Floating";

interface Props {
  proposal: FixProposal;
  busy: boolean;
  error: string | undefined;
  onApply: (edited: string[] | null, force: boolean) => void;
  /** Regenerates, switching to `mode` when given. */
  onRegenerate: (mode?: FixMode) => void;
  onReject: () => void;
  /** Opens a cited file, or a web page when given its address. */
  onOpenCitation: (pathOrUrl: string) => void;
  /** Searches the web for what a placeholder asks for, given the text around it. */
  onSearch: (need: string, passage: string) => void;
}

const PLACEHOLDER_RE = /【待补充[:：]?([^】]*)】/g;

interface Need {
  need: string;
  /** The text around the placeholder, so the AI knows what it is about. */
  passage: string;
  /** The project's own data, which only the report's authors have; not looked up online. */
  fromAuthors: boolean;
}

/** What each "【待补充：…】" in the proposal asks for, without repeats. */
function placeholderNeeds(proposal: FixProposal): Need[] {
  const needs = new Map<string, Need>();
  for (const p of proposal.paragraphs)
    for (const m of p.new.matchAll(PLACEHOLDER_RE)) {
      const need = m[1].trim() || "待补充内容";
      const at = m.index ?? 0;
      const passage = p.new.slice(Math.max(0, at - 160), at + m[0].length + 60);
      if (!needs.has(need)) needs.set(need, { need, passage, fromAuthors: need.includes("编制单位") });
    }
  return [...needs.values()];
}

/** Lists what the model could not fill in, each with a web search. */
function Placeholders({ needs, onSearch }: { needs: Need[]; onSearch: (need: string, passage: string) => void }) {
  return (
    <div className="fix-placeholders">
      <div className="head">
        <AlertTriangle size={13} /> 修改里有待补充的内容，补充成实际内容后才能应用
      </div>
      {needs.map(({ need, passage, fromAuthors }) => (
        <div key={need} className="need">
          <span className="text">{need.replace(/^需编制单位提供[:：]?/, "")}</span>
          {fromAuthors ? (
            <span className="tag" title="本项目自身的数据，文中其他章节没有，网上也查不到，请向编制单位索取后填写">
              需编制单位提供
            </span>
          ) : (
            <button type="button" className="btn sm" onClick={() => onSearch(need, passage)}>
              <Globe size={12} /> 查找资料
            </button>
          )}
        </div>
      ))}
    </div>
  );
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
  const { passages, related, examples, profile } = proposal.context;
  const context = [passages && `${passages} 段资料`, related && `本文 ${related} 段相关内容`, examples && `${examples} 条历史案例`, profile && "审稿人画像"].filter(Boolean);
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

function CitationChips({ citations, onOpen }: { citations: Citation[]; onOpen: (pathOrUrl: string) => void }) {
  const [hover, setHover] = useState<{ anchor: HTMLElement; citation: Citation } | null>(null);
  return (
    <div className="cite-chips">
      {citations.map((c) => (
        <button
          key={`${c.n}-${c.chunkId}`}
          type="button"
          className="cite-chip"
          title={c.url}
          onClick={() => onOpen(c.url ?? c.storedPath)}
          onMouseEnter={(e) => setHover({ anchor: e.currentTarget, citation: c })}
          onMouseLeave={() => setHover(null)}
          onFocus={(e) => setHover({ anchor: e.currentTarget, citation: c })}
          onBlur={() => setHover(null)}
        >
          <span className="n">[{c.n}]</span>
          {c.url && <Globe size={11} />}
          <span className="text">
            {c.url ? c.title || c.fileName : c.fileName}
            {c.headingPath.length > 0 && ` · ${c.headingPath.join(" › ")}`}
          </span>
        </button>
      ))}
      {hover && (
        <Floating anchor={hover.anchor} width={320} className="hover-card">
          <div className="hover-title">
            {hover.citation.url ? <Globe size={13} /> : <FileText size={13} />} {hover.citation.title}
          </div>
          {hover.citation.url && <div className="hover-path">{hover.citation.fileName}</div>}
          {hover.citation.headingPath.length > 0 && <div className="hover-path">{hover.citation.headingPath.join(" › ")}</div>}
          <div className="hover-text">{hover.citation.text}</div>
          <div className="hover-hint">{hover.citation.url ? "联网查到的网页，点击在浏览器中打开" : "点击用默认程序打开"}</div>
        </Floating>
      )}
    </div>
  );
}

/** An AI fix inside a comment card: diff, judgement, sources and actions. */
export const FixResult = memo(function FixResult({ proposal, busy, error, onApply, onRegenerate, onReject, onOpenCitation, onSearch }: Props) {
  const [details, setDetails] = useState(false);
  const [editing, setEditing] = useState<string[] | null>(null);
  const [confirming, setConfirming] = useState(false);
  const { judge } = proposal;
  const blocked = judge !== null && !judge.passed;
  const needs = hasPlaceholder(proposal) ? placeholderNeeds(proposal) : [];
  const suggested = proposal.paragraphs.map((p) => p.new);
  // Placeholders must be replaced by hand: the edit has to change the text and leave none behind.
  const unfilled = editing !== null && needs.length > 0 && (editing.some((t) => t.includes(PLACEHOLDER)) || editing.every((t, i) => t === suggested[i]));

  return (
    <div className="fix-result" onClick={(e) => e.stopPropagation()}>
      <div className="fix-head">
        <ConfidencePill proposal={proposal} />
        {judge?.category && <span className="tag">{judge.category}</span>}
        {proposal.mode === "rewrite" && <span className="tag">重写</span>}
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
      {needs.length > 0 && <Placeholders needs={needs} onSearch={onSearch} />}

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
            <button
              type="button"
              className="btn sm primary"
              disabled={busy || unfilled}
              title={unfilled ? "先把“【待补充…】”改成实际内容" : undefined}
              onClick={() => onApply(editing, true)}
            >
              {busy && <span className="spinner sm" />}应用修改
            </button>
            <button type="button" className="btn sm" disabled={busy} onClick={() => setEditing(null)}>
              取消
            </button>
          </>
        ) : (
          <>
            {needs.length > 0 ? (
              <button type="button" className="btn sm primary" disabled={busy} onClick={() => setEditing(suggested)}>
                补充后应用
              </button>
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
                <button type="button" className="btn sm" disabled={busy} onClick={() => setEditing(suggested)}>
                  编辑
                </button>
              </>
            )}
            <button type="button" className="btn sm" disabled={busy} onClick={() => onRegenerate()} title="按当前的修改方向重新生成">
              <RefreshCw size={12} /> 重新生成
            </button>
            {proposal.mode === "rewrite" ? (
              <button type="button" className="btn sm" disabled={busy} title="只改批注指出的问题" onClick={() => onRegenerate("fix")}>
                <Sparkles size={12} /> 改用修复
              </button>
            ) : (
              <button type="button" className="btn sm" disabled={busy} title="原文本身不对时，整段重写" onClick={() => onRegenerate("rewrite")}>
                <PenLine size={12} /> 改用重写
              </button>
            )}
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
