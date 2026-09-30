import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { AlertCircle, Columns2, LayoutList, ListFilter, MessageSquareText, Sparkles, X } from "lucide-react";
import type { Backend } from "../api";
import type { CommentLayout } from "../hooks/useCommentLayout";
import type { Notify, RunEdit } from "../hooks/useDocumentSession";
import { useFixes } from "../hooks/useFixes";
import type { AuthorView, CommentView, FixSelection } from "../types";
import { avatarColor, avatarText, errorMessage, hasPlaceholder } from "../util";
import { AuthorPopover } from "./AuthorPopover";
import { CommentCard, type CardActions } from "./CommentCard";
import { ConfirmButton } from "./ConfirmButton";
import type { MarginHost } from "./DocumentView";
import { MarginComments } from "./MarginComments";

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
  /** Opens the web search for what a "【待补充…】" in a comment's fix asks for. */
  onSearch: (comment: CommentView, need: string, passage: string) => void;
  notify: Notify;
  /** `margin`: cards are drawn in the document's page margin instead of this panel (C-4). */
  layout: CommentLayout;
  onLayout: (layout: CommentLayout) => void;
  /** The page margin, while the document view shows one. */
  marginHost: MarginHost | null;
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
  onSearch,
  notify,
  layout,
  onLayout,
  marginHost,
}: Props) {
  const [status, setStatus] = useState<Status>("all");
  const [selectedKeys, setSelectedKeys] = useState<ReadonlySet<string>>(() => new Set());
  const [popover, setPopover] = useState<{ author: AuthorView; anchor: HTMLElement } | null>(null);
  /** Reviewer chips in the margin bar are folded away behind a button. */
  const [chipsOpen, setChipsOpen] = useState(false);
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
  const visible = useMemo(
    () =>
      threads.filter(
        (t) =>
          // A just-applied fix resolves its comment; keep the card in the 未解决 view while it shows 已应用.
          (status === "all" || (status === "done") === t.done || entries.get(t.id)?.kind === "applied") &&
          (!activeKeys || activeKeys.has(keyOf(t.author))),
      ),
    [threads, status, entries, activeKeys, keyOf],
  );
  const batchable = visible.filter((t) => {
    const e = entries.get(t.id);
    return !t.done && (!e || e.kind === "failed");
  });
  const passedCount = [...entries.values()].filter((e) => e.kind === "ready" && e.proposal.judge?.passed && !hasPlaceholder(e.proposal)).length;
  const batchDone = batch ? batch.filter((id) => entries.get(id)?.kind !== "running").length : 0;
  const batchRunning = batch !== null && batchDone < batch.length;

  // Keep the selected card in view when it was selected from the document.
  useEffect(() => {
    if (!activeId || layout !== "dense") return;
    listRef.current
      ?.querySelector<HTMLElement>(`[data-comment="${CSS.escape(activeId)}"]`)
      ?.scrollIntoView({ block: "nearest", behavior: "smooth" });
  }, [activeId, layout]);

  const toggleKey = (key: string) =>
    setSelectedKeys((prev) => {
      const next = new Set(activeKeys ? prev : []);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });

  const { fix, setDirection, apply, reject } = fixes;
  const actions: CardActions = useMemo(
    () => ({
      select: onSelect,
      fix,
      setDirection,
      apply,
      reject,
      reply: (c, text) =>
        edit(() => backend.addCommentReply(docId, c.id, text)).then(
          () => true,
          (e) => {
            notify(`回复失败：${errorMessage(e)}`, true);
            return false;
          },
        ),
      search: onSearch,
      toggleDone: (c) =>
        void edit(() => backend.setCommentDone(docId, c.id, !c.done)).catch((e) => notify(`操作失败：${errorMessage(e)}`, true)),
      openAuthor: (name, anchor) => {
        const author = authorsByName.get(name);
        if (author) setPopover((p) => (p?.anchor === anchor ? null : { author, anchor }));
      },
      openCitation: (path) => void backend.openPath(path).catch((e) => notify(`无法打开文件：${errorMessage(e)}`, true)),
    }),
    [onSelect, fix, setDirection, apply, reject, edit, backend, docId, notify, authorsByName, onSearch],
  );
  const closePopover = useCallback(() => setPopover(null), []);

  const startBatch = () => void fixes.runBatch(batchable.map((t) => t.id));

  const margin = layout === "margin";
  const layoutToggle = (
    <button
      type="button"
      className="icon-btn sm layout-toggle"
      title={margin ? "切换为密集列表（批注集中在右侧栏）" : "切换为经典 Word 排版（批注在页边，与正文同高）"}
      onClick={() => onLayout(margin ? "dense" : "margin")}
    >
      {margin ? <LayoutList size={15} /> : <Columns2 size={15} />}
    </button>
  );

  const batchButton =
    batchable.length > LARGE_BATCH ? (
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
    );

  const banner = needsModel && (
    <div className="panel-banner">
      <AlertCircle size={14} />
      <span>尚未配置大语言模型 —</span>
      <button type="button" className="link-btn" onClick={onOpenSettings}>
        去设置
      </button>
    </div>
  );

  const statusTabs = (
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
  );

  const chips = authorChips.length > 1 && (
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
  );

  const batchBar = (batch || passedCount > 0) && (
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
  );

  const authorPopover = popover && (
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
  );

  if (margin) {
    if (!marginHost) return authorPopover;
    const filtering = activeKeys !== null;
    return (
      <>
        {createPortal(
          <div className="margin-bar">
            <div className="row">
              <h2>批注</h2>
              <span className="count" title={`${openCount} 条未解决 · 共 ${threads.length} 条`}>
                {openCount} 未解决 · 共 {threads.length}
              </span>
              <span className="spacer" />
              {batchButton}
              {layoutToggle}
            </div>
            <div className="row">
              {statusTabs}
              {chips && (
                <button
                  type="button"
                  className={`icon-btn sm${chipsOpen || filtering ? " on" : ""}`}
                  title={chipsOpen ? "收起审稿人筛选" : "按审稿人筛选"}
                  onClick={() => setChipsOpen((v) => !v)}
                >
                  <ListFilter size={15} />
                  {filtering && <span className="dot" />}
                </button>
              )}
            </div>
            {chipsOpen && chips}
            {banner}
            {batchBar}
            {visible.length === 0 && (
              <div className="margin-empty">{threads.length === 0 ? "这份文档没有批注" : "没有符合筛选条件的批注"}</div>
            )}
          </div>,
          marginHost.bar,
        )}
        <MarginComments
          host={marginHost}
          comments={comments}
          threads={visible}
          replies={replies}
          activeId={activeId}
          selection={selection}
          authorsByName={authorsByName}
          nameOf={nameOf}
          entries={entries}
          directions={fixes.directions}
          actions={actions}
        />
        {authorPopover}
      </>
    );
  }

  return (
    <>
      <div className="panel-head">
        <h2>批注</h2>
        <span className="count">
          {openCount} 条未解决 · 共 {threads.length} 条
        </span>
        <span className="spacer" />
        {batchButton}
        {layoutToggle}
      </div>
      {banner}
      <div className="filters">
        {statusTabs}
        {chips}
      </div>
      {batchBar}
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
            direction={fixes.directions.get(t.id) ?? ""}
            actions={actions}
          />
        ))}
      </div>
      {authorPopover}
    </>
  );
});
