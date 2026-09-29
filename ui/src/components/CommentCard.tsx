import { memo, type MouseEvent } from "react";
import { ArrowRight, Check, CheckCircle2, Circle, RotateCw, Sparkles } from "lucide-react";
import type { FixEntry, FixActions } from "../hooks/useFixes";
import { FIX_STAGE } from "../labels";
import type { AuthorView, CommentView, FixStage } from "../types";
import { avatarColor, avatarText, formatDate } from "../util";
import { FixResult } from "./FixResult";

/** Callbacks shared by every card; the object is stable so cards stay memoized. */
export interface CardActions extends FixActions {
  select: (comment: CommentView) => void;
  toggleDone: (comment: CommentView) => void;
  openAuthor: (author: string, anchor: HTMLElement) => void;
  openCitation: (path: string) => void;
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
  actions: CardActions;
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

export const CommentCard = memo(function CommentCard({ comment: c, replies, active, author, nameOf, fix, actions }: Props) {
  const reviewer = author?.reviewer ?? null;
  const shown = reviewer?.name ?? c.author;

  let fixArea = null;
  if (fix?.kind === "running") fixArea = <FixProgressLine stage={fix.stage} />;
  else if (fix?.kind === "ready")
    fixArea = (
      <FixResult
        proposal={fix.proposal}
        busy={!!fix.busy}
        error={fix.error}
        onApply={(edited, force) => void actions.apply(c.id, fix.proposal, edited, force)}
        onRegenerate={() => actions.fix(c.id)}
        onReject={() => actions.reject(c.id, fix.proposal)}
        onOpenCitation={actions.openCitation}
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
      </div>
    );
  else if (!c.done)
    fixArea = (
      <div className="card-actions" onClick={stop}>
        <button type="button" className="btn sm ai" onClick={() => actions.fix(c.id)}>
          <Sparkles size={13} /> AI 修复
        </button>
      </div>
    );

  return (
    <div
      data-comment={c.id}
      className={`comment-card${active ? " active" : ""}${c.done ? " done" : ""}`}
      onClick={() => actions.select(c)}
    >
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
      {fixArea}
    </div>
  );
});
