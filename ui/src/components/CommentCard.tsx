import { memo, useState, type MouseEvent } from "react";
import { ArrowRight, Check, CheckCircle2, Circle, Compass, MessageSquareReply, PenLine, RotateCw, Sparkles, TextSelect } from "lucide-react";
import type { FixEntry, FixActions } from "../hooks/useFixes";
import { FIX_STAGE } from "../labels";
import type { AuthorView, CommentView, FixSelection, FixStage } from "../types";
import { avatarColor, avatarText, formatDate, hasPlaceholder } from "../util";
import { FixResult } from "./FixResult";

/** Callbacks shared by every card; the object is stable so cards stay memoized. */
export interface CardActions extends FixActions {
  select: (comment: CommentView) => void;
  toggleDone: (comment: CommentView) => void;
  /** Adds a reply to the thread; resolves false when it failed (the error is shown as a toast). */
  reply: (comment: CommentView, text: string) => Promise<boolean>;
  openAuthor: (author: string, anchor: HTMLElement) => void;
  openCitation: (path: string) => void;
  /** Looks up what a "【待补充…】" asks for on the web. */
  search: (comment: CommentView, need: string) => void;
}

interface Props {
  comment: CommentView;
  replies: CommentView[] | undefined;
  active: boolean;
  /** The comment's signature and the reviewer it maps to; undefined until authors load. */
  author: AuthorView | undefined;
  /** Display name of any signature (the reviewer's name when mapped). */
  nameOf: (author: string) => string;
  fix: FixEntry | undefined;
  /** Text selected in the document; only the active card gets it. */
  selection: FixSelection | null;
  /** The revision direction typed for this comment. */
  direction: string;
  actions: CardActions;
}

function rangeLabel(s: FixSelection): string {
  const [a, b] = [s.startParagraph + 1, s.endParagraph + 1];
  return a === b ? `第 ${a} 段` : `第 ${a}–${b} 段`;
}

/** Runs a fix on the selected paragraphs instead of the highlighted ones. */
function SelectionFix({ selection, onFix }: { selection: FixSelection; onFix: () => void }) {
  const preview = selection.text.length > 40 ? `${selection.text.slice(0, 40)}…` : selection.text;
  return (
    <button
      type="button"
      className="btn sm"
      title={`审稿人的标注没选全时，按你在正文中选中的范围修改：\n${preview}`}
      onClick={onFix}
    >
      <TextSelect size={13} /> 按选区修复（{rangeLabel(selection)}）
    </button>
  );
}

const STAGES: FixStage[] = ["context", "retrieve", "generate", "judge"];

const stop = (e: MouseEvent) => e.stopPropagation();

function FixProgressLine({ stage }: { stage: FixStage | null }) {
  const at = stage ? STAGES.indexOf(stage) : -1;
  return (
    <div className="fix-progress">
      <span className="spinner sm" />
      <span>{stage ? FIX_STAGE[stage] : "排队中"}…</span>
      <span className="stage-dots" aria-hidden>
        {STAGES.map((s, i) => (
          <i key={s} className={i < at ? "done" : i === at ? "now" : ""} />
        ))}
      </span>
    </div>
  );
}

/** The short state shown on a collapsed card. */
function fixBadge(fix: FixEntry | undefined): { text: string; tone: string } | null {
  switch (fix?.kind) {
    case "running":
      return { text: "生成中", tone: "neutral" };
    case "ready":
      if (hasPlaceholder(fix.proposal)) return { text: "待补充", tone: "warn" };
      return fix.proposal.judge?.passed ? { text: "待应用", tone: "positive" } : { text: "待确认", tone: "warn" };
    case "applied":
      return { text: "已应用", tone: "positive" };
    case "failed":
      return { text: "失败", tone: "negative" };
    default:
      return null;
  }
}

/** Steers the next fix; kept per comment while the document is open. */
function DirectionBox({ value, onChange, running }: { value: string; onChange: (text: string) => void; running: boolean }) {
  return (
    <div className="direction-box" onClick={stop}>
      <label className="direction-label">
        <Compass size={12} /> 修改方向
        <span className="muted">{running ? "（下次生成时生效）" : "（可选，生成和重新生成时都会带上）"}</span>
      </label>
      <textarea
        className="textarea"
        rows={2}
        value={value}
        placeholder="例如：按 2024 年统计口径表述；不要删掉第二句；语气更正式"
        onChange={(e) => onChange(e.target.value)}
      />
    </div>
  );
}

function ReplyBox({ onSend, onCancel }: { onSend: (text: string) => Promise<boolean>; onCancel: () => void }) {
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const send = async () => {
    setBusy(true);
    const ok = await onSend(text);
    setBusy(false);
    if (ok) onCancel();
  };
  return (
    <div className="reply-box" onClick={stop}>
      <textarea
        className="textarea"
        rows={2}
        autoFocus
        value={text}
        placeholder="回复这条批注，保存后在 Word 中可见"
        onChange={(e) => setText(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && (e.ctrlKey || e.metaKey) && text.trim()) void send();
          if (e.key === "Escape") onCancel();
        }}
      />
      <div className="row">
        <button type="button" className="btn sm primary" disabled={busy || !text.trim()} onClick={() => void send()}>
          {busy && <span className="spinner sm" />}回复
        </button>
        <button type="button" className="btn sm ghost" disabled={busy} onClick={onCancel}>
          取消
        </button>
        <span className="muted">Ctrl+Enter 发送</span>
      </div>
    </div>
  );
}

export const CommentCard = memo(function CommentCard({
  comment: c,
  replies,
  active,
  author,
  nameOf,
  fix,
  selection,
  direction,
  actions,
}: Props) {
  const [showDirection, setShowDirection] = useState(false);
  const [replying, setReplying] = useState(false);
  const reviewer = author?.reviewer ?? null;
  const shown = reviewer?.name ?? c.author;

  if (!active) {
    const badge = fixBadge(fix);
    return (
      <div
        data-comment={c.id}
        className={`comment-card collapsed${c.done ? " done" : ""}`}
        onClick={() => actions.select(c)}
        title={c.text}
      >
        <span className="avatar" style={{ background: avatarColor(shown) }}>
          {avatarText(shown)}
        </span>
        <span className="who">{shown || "未署名"}</span>
        <span className="snippet">{c.text}</span>
        {replies && replies.length > 0 && <span className="muted n-replies">{replies.length} 条回复</span>}
        {badge && <span className={`pill ${badge.tone} sm`}>{badge.text}</span>}
        {c.done && <CheckCircle2 size={13} className="done-mark" aria-label="已解决" />}
      </div>
    );
  }

  const directionOpen = showDirection || direction !== "";
  const running = fix?.kind === "running";

  let fixArea = null;
  if (running) fixArea = <FixProgressLine stage={fix.stage} />;
  else if (fix?.kind === "ready")
    fixArea = (
      <FixResult
        proposal={fix.proposal}
        busy={!!fix.busy}
        error={fix.error}
        onApply={(edited, force) => void actions.apply(c.id, fix.proposal, edited, force)}
        onRegenerate={(mode) => actions.fix(c.id, { mode, selection: selection ?? undefined })}
        onReject={() => actions.reject(c.id, fix.proposal)}
        onOpenCitation={actions.openCitation}
        onSearch={(need) => actions.search(c, need)}
      />
    );
  else if (fix?.kind === "applied")
    fixArea = (
      <div className="fix-applied">
        <Check size={14} /> 已应用
      </div>
    );
  else if (fix?.kind === "failed")
    fixArea = (
      <div className="fix-failed" onClick={stop}>
        <span>{fix.error}</span>
        <button type="button" className="btn sm" onClick={() => actions.fix(c.id)}>
          <RotateCw size={12} /> 重试
        </button>
        {selection && <SelectionFix selection={selection} onFix={() => actions.fix(c.id, { selection })} />}
      </div>
    );
  else if (!c.done)
    fixArea = (
      <div className="card-actions" onClick={stop}>
        <button type="button" className="btn sm ai" title="只改批注指出的问题，改动越小越好" onClick={() => actions.fix(c.id, { selection: null, mode: "fix" })}>
          <Sparkles size={13} /> AI 修复
        </button>
        <button
          type="button"
          className="btn sm"
          title="原文本身就不对时使用：结合批注和修改方向重写这些段落"
          onClick={() => actions.fix(c.id, { selection: null, mode: "rewrite" })}
        >
          <PenLine size={13} /> AI 重写
        </button>
        {selection ? (
          <SelectionFix selection={selection} onFix={() => actions.fix(c.id, { selection })} />
        ) : (
          <span className="card-hint">标注没选全？先在正文中选中要改的文字</span>
        )}
      </div>
    );

  return (
    <div data-comment={c.id} className={`comment-card active${c.done ? " done" : ""}`} onClick={() => actions.select(c)}>
      <div className="comment-meta">
        <span className="avatar lg" style={{ background: avatarColor(shown) }}>
          {avatarText(shown)}
        </span>
        <button
          type="button"
          className="who author-btn"
          title="设置这个署名对应的审稿人"
          onClick={(e) => {
            e.stopPropagation();
            actions.openAuthor(c.author, e.currentTarget);
          }}
        >
          {reviewer && reviewer.name !== c.author ? (
            <>
              <span className="alias">{c.author || "未署名"}</span>
              <ArrowRight size={11} className="arrow" />
              <span>{reviewer.name}</span>
            </>
          ) : (
            c.author || "未署名"
          )}
        </button>
        {author && !reviewer && <span className="badge unknown">未识别</span>}
        <span className="spacer" />
        <span className="when">{formatDate(c.date)}</span>
        <button
          type="button"
          className={`resolve-btn${c.done ? " on" : ""}`}
          title={c.done ? "重新打开" : "标记为已解决"}
          onClick={(e) => {
            e.stopPropagation();
            actions.toggleDone(c);
          }}
        >
          {c.done ? <CheckCircle2 size={14} /> : <Circle size={14} />}
          {c.done && "已解决"}
        </button>
      </div>
      {c.quote && <div className="comment-quote">{c.quote}</div>}
      <div className="comment-text">{c.text}</div>
      {replies?.map((r) => {
        const name = nameOf(r.author);
        return (
          <div key={r.id} className="reply">
            <div className="comment-meta">
              <span className="avatar" style={{ background: avatarColor(name) }}>
                {avatarText(name)}
              </span>
              <span className="who">{name}</span>
              <span className="when">{formatDate(r.date)}</span>
            </div>
            <div className="comment-text">{r.text}</div>
          </div>
        );
      })}
      {directionOpen && !c.done && <DirectionBox value={direction} running={running} onChange={(t) => actions.setDirection(c.id, t)} />}
      {fixArea}
      {replying ? (
        <ReplyBox onSend={(text) => actions.reply(c, text)} onCancel={() => setReplying(false)} />
      ) : (
        <div className="card-foot" onClick={stop}>
          <button type="button" className="link-btn" onClick={() => setReplying(true)}>
            <MessageSquareReply size={13} /> 回复
          </button>
          {!c.done && !directionOpen && (
            <button type="button" className="link-btn" onClick={() => setShowDirection(true)}>
              <Compass size={13} /> 修改方向
            </button>
          )}
        </div>
      )}
    </div>
  );
});
