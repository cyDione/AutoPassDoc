import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { AlertCircle, MessageSquareText, Sparkles, X } from "lucide-react";
import type { Backend } from "../api";
import type { Notify, RunEdit } from "../hooks/useDocumentSession";
import { useFixes } from "../hooks/useFixes";
import type { AuthorView, CommentView, FixSelection } from "../types";
import { avatarColor, avatarText, errorMessage } from "../util";
import { AuthorPopover } from "./AuthorPopover";
import { CommentCard, type CardActions } from "./CommentCard";
import { ConfirmButton } from "./ConfirmButton";

type Status = "open" | "all" | "done";

/** Batches this large ask for confirmation first. */
const LARGE_BATCH = 20;

interface Props {
  backend: Backend;
  docId: number;
  comments: CommentView[];
  authors: AuthorView[];
  activeId: string | null;
  /** Text selected in the document, offered to the active comment's fix. */
  selection: FixSelection | null;
  /** No chat model is configured, so fixes cannot run. */
  needsModel: boolean;
  onSelect: (comment: CommentView) => void;
  edit: RunEdit;
  onAuthors: (authors: AuthorView[]) => void;
  onOpenSettings: () => void;
  notify: Notify;
}

export const CommentsPanel = memo(function CommentsPanel({
  backend,
  docId,
  comments,
  authors,
  activeId,
  selection,
  needsModel,
  onSelect,
  edit,
  onAuthors,
  onOpenSettings,
  notify,
}: Props) {
  const [status, setStatus] = useState<Status>("all");
  const [selectedKeys, setSelectedKeys] = useState<ReadonlySet<string>>(() => new Set());
  const [popover, setPopover] = useState<{ author: AuthorView; anchor: HTMLElement } | null>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const fixes = useFixes(backend, docId, edit, notify);
  const { entries, batch, applyingAll } = fixes;

  const authorsByName = useMemo(() => new Map(authors.map((a) => [a.author, a])), [authors]);
  /** Filter key: one per reviewer, or per signature when unmapped. */
  const keyOf = useCallback(
    (author: string) => {
      const r = authorsByName.get(author)?.reviewer;
      return r ? `r:${r.id}` : `a:${author}`;
    },
    [authorsByName],
  );
  const nameOf = useCallback((author: string) => authorsByName.get(author)?.reviewer?.name ?? author, [authorsByName]);

  const { threads, replies, openCount } = useMemo(() => {
    const replies = new Map<string, CommentView[]>();
    const threads: CommentView[] = [];
    for (const c of comments) {
      if (c.parentId) {
        const list = replies.get(c.parentId) ?? [];
        list.push(c);
        replies.set(c.parentId, list);
      } else {
        threads.push(c);
      }
    }
    threads.sort((a, b) => (a.paragraphIndex ?? Infinity) - (b.paragraphIndex ?? Infinity));
    return { threads, replies, openCount: threads.filter((t) => !t.done).length };
  }, [comments]);

  const authorChips = useMemo(() => {
    const chips = new Map<string, { label: string; count: number }>();
    for (const t of threads) {
      const key = keyOf(t.author);
      const chip = chips.get(key);
      if (chip) chip.count++;
      else chips.set(key, { label: nameOf(t.author), count: 1 });
    }
    return [...chips.entries()].sort((a, b) => b[1].count - a[1].count);
  }, [threads, keyOf, nameOf]);

  // Mappings can change under a filter; ignore keys that no longer exist.
  const activeKeys = authorChips.some(([key]) => selectedKeys.has(key)) ? selectedKeys : null;
  const visible = threads.filter(
    (t) =>
      // A just-applied fix resolves its comment; keep the card in the 未解决 view while it shows 已应用.
      (status === "all" || (status === "done") === t.done || entries.get(t.id)?.kind === "applied") &&
      (!activeKeys || activeKeys.has(keyOf(t.author))),
  );
  const batchable = visible.filter((t) => {
    const e = entries.get(t.id);
    return !t.done && (!e || e.kind === "failed");
  });
  const passedCount = [...entries.values()].filter((e) => e.kind === "ready" && e.proposal.judge?.passed).length;
  const batchDone = batch ? batch.filter((id) => entries.get(id)?.kind !== "running").length : 0;
  const batchRunning = batch !== null && batchDone < batch.length;

  // Keep the selected card in view when it was selected from the document.
  useEffect(() => {
    if (!activeId) return;
    listRef.current
      ?.querySelector<HTMLElement>(`[data-comment="${CSS.escape(activeId)}"]`)
      ?.scrollIntoView({ block: "nearest", behavior: "smooth" });
  }, [activeId]);

  const toggleKey = (key: string) =>
    setSelectedKeys((prev) => {
      const next = new Set(activeKeys ? prev : []);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });

  const { fix, apply, reject } = fixes;
  const actions: CardActions = useMemo(
    () => ({
      select: onSelect,
      fix,
      apply,
      reject,
      toggleDone: (c) =>
        void edit(() => backend.setCommentDone(docId, c.id, !c.done)).catch((e) => notify(`操作失败：${errorMessage(e)}`, true)),
      openAuthor: (name, anchor) => {
        const author = authorsByName.get(name);
        if (author) setPopover((p) => (p?.anchor === anchor ? null : { author, anchor }));
      },
      openCitation: (path) => void backend.openPath(path).catch((e) => notify(`无法打开文件：${errorMessage(e)}`, true)),
    }),
    [onSelect, fix, apply, reject, edit, backend, docId, notify, authorsByName],
  );
  const closePopover = useCallback(() => setPopover(null), []);

  const startBatch = () => void fixes.runBatch(batchable.map((t) => t.id));

  return (
    <>
      <div className="panel-head">
        <h2>批注</h2>
        <span className="count">
          {openCount} 条未解决 · 共 {threads.length} 条
        </span>
        <span className="spacer" />
        {batchable.length > LARGE_BATCH ? (
          <ConfirmButton
            className="btn sm"
            disabled={batchRunning}
            title="为当前筛选出的未解决批注逐条生成修改"
            confirmLabel={`确认修复 ${batchable.length} 条？`}
            onConfirm={startBatch}
          >
            <Sparkles size={13} /> 批量修复
          </ConfirmButton>
        ) : (
          <button
            type="button"
            className="btn sm"
            disabled={batchRunning || batchable.length === 0}
            title={batchable.length ? `为当前筛选出的 ${batchable.length} 条未解决批注生成修改` : "当前筛选下没有待修复的批注"}
            onClick={startBatch}
          >
            <Sparkles size={13} /> 批量修复
          </button>
        )}
      </div>
      {needsModel && (
        <div className="panel-banner">
          <AlertCircle size={14} />
          <span>尚未配置大语言模型 —</span>
          <button type="button" className="link-btn" onClick={onOpenSettings}>
            去设置
          </button>
        </div>
      )}
      <div className="filters">
        <div className="segmented" role="tablist">
          {(
            [
              ["all", "全部"],
              ["open", "未解决"],
              ["done", "已解决"],
            ] as const
          ).map(([key, label]) => (
            <button key={key} className={status === key ? "on" : ""} onClick={() => setStatus(key)}>
              {label}
            </button>
          ))}
        </div>
        {authorChips.length > 1 && (
          <div className="chips">
            {authorChips.map(([key, { label, count }]) => (
              <button key={key} className={`chip${activeKeys?.has(key) ? " on" : ""}`} onClick={() => toggleKey(key)}>
                <span className="avatar" style={{ background: avatarColor(label) }}>
                  {avatarText(label)}
                </span>
                {label}
                <span className="n">{count}</span>
              </button>
            ))}
          </div>
        )}
      </div>
      {(batch || passedCount > 0) && (
        <div className="batch-bar">
          <div className="row">
            <span className="label">
              {batch ? (batchRunning ? `批量修复 ${batchDone}/${batch.length}` : `批量修复完成 ${batch.length} 条`) : "待应用的修改"}
            </span>
            {passedCount > 0 && <span className="muted">{passedCount} 条达标</span>}
            <span className="spacer" />
            {passedCount > 0 && (
              <button type="button" className="btn sm primary" disabled={applyingAll !== null} onClick={() => void fixes.applyPassed()}>
                {applyingAll ? `正在应用 ${applyingAll.done}/${applyingAll.total}` : "应用全部达标"}
              </button>
            )}
            {batch && !batchRunning && (
              <button type="button" className="icon-btn sm" title="关闭" onClick={fixes.closeBatch}>
                <X size={14} />
              </button>
            )}
          </div>
          {batch && (
            <div className="progress">
              <span style={{ width: `${(batchDone / batch.length) * 100}%` }} />
            </div>
          )}
        </div>
      )}
      <div className="comment-list scroll" ref={listRef}>
        {visible.length === 0 && (
          <div className="panel-empty">
            <MessageSquareText size={22} strokeWidth={1.5} />
            <div>{threads.length === 0 ? "这份文档没有批注" : "没有符合筛选条件的批注"}</div>
          </div>
        )}
        {visible.map((t) => (
          <CommentCard
            key={t.id}
            comment={t}
            replies={replies.get(t.id)}
            active={t.id === activeId}
            selection={t.id === activeId ? selection : null}
            author={authorsByName.get(t.author)}
            nameOf={nameOf}
            fix={entries.get(t.id)}
            actions={actions}
          />
        ))}
      </div>
      {popover && (
        <AuthorPopover
          key={popover.author.author}
          backend={backend}
          docId={docId}
          author={popover.author}
          anchor={popover.anchor}
          edit={edit}
          onAuthors={onAuthors}
          onClose={closePopover}
          notify={notify}
        />
      )}
    </>
  );
});
