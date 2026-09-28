import { memo, useEffect, useMemo, useRef, useState } from "react";
import { CheckCircle2, MessageSquareText } from "lucide-react";
import type { CommentView } from "../types";
import { avatarColor, avatarText, formatDate } from "../util";

type Status = "open" | "all" | "done";

interface Props {
  comments: CommentView[];
  activeId: string | null;
  onSelect: (comment: CommentView) => void;
}

export const CommentsPanel = memo(function CommentsPanel({ comments, activeId, onSelect }: Props) {
  const [status, setStatus] = useState<Status>("all");
  const [authors, setAuthors] = useState<Set<string>>(new Set());
  const listRef = useRef<HTMLDivElement>(null);

  const { threads, replies, authorCounts, openCount } = useMemo(() => {
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
    const authorCounts = new Map<string, number>();
    for (const t of threads) authorCounts.set(t.author, (authorCounts.get(t.author) ?? 0) + 1);
    const openCount = threads.filter((t) => !t.done).length;
    return { threads, replies, authorCounts, openCount };
  }, [comments]);

  const visible = threads.filter(
    (t) =>
      (status === "all" || (status === "done") === t.done) && (authors.size === 0 || authors.has(t.author)),
  );

  // Keep the selected card in view when it was selected from the document.
  useEffect(() => {
    if (!activeId) return;
    listRef.current
      ?.querySelector<HTMLElement>(`[data-comment="${CSS.escape(activeId)}"]`)
      ?.scrollIntoView({ block: "nearest", behavior: "smooth" });
  }, [activeId]);

  const toggleAuthor = (name: string) =>
    setAuthors((prev) => {
      const next = new Set(prev);
      if (next.has(name)) next.delete(name);
      else next.add(name);
      return next;
    });

  return (
    <>
      <div className="panel-head">
        <h2>批注</h2>
        <span className="count">
          {openCount} 条未解决 · 共 {threads.length} 条
        </span>
      </div>
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
        {authorCounts.size > 1 && (
          <div className="chips">
            {[...authorCounts.entries()]
              .sort((a, b) => b[1] - a[1])
              .map(([name, n]) => (
                <button key={name} className={`chip${authors.has(name) ? " on" : ""}`} onClick={() => toggleAuthor(name)}>
                  <span className="avatar" style={{ background: avatarColor(name) }}>
                    {avatarText(name)}
                  </span>
                  {name}
                  <span className="n">{n}</span>
                </button>
              ))}
          </div>
        )}
      </div>
      <div className="comment-list scroll" ref={listRef}>
        {visible.length === 0 && (
          <div className="panel-empty">
            <MessageSquareText size={22} strokeWidth={1.5} />
            <div>{threads.length === 0 ? "这份文档没有批注" : "没有符合筛选条件的批注"}</div>
          </div>
        )}
        {visible.map((t) => (
          <div
            key={t.id}
            data-comment={t.id}
            className={`comment-card${t.id === activeId ? " active" : ""}${t.done ? " done" : ""}`}
            onClick={() => onSelect(t)}
          >
            <div className="comment-meta">
              <span className="avatar lg" style={{ background: avatarColor(t.author) }}>
                {avatarText(t.author)}
              </span>
              <span className="who">{t.author || "未署名"}</span>
              <span className="when">{formatDate(t.date)}</span>
              <span className="spacer" />
              {t.done && (
                <span className="badge done">
                  <CheckCircle2 size={11} /> 已解决
                </span>
              )}
            </div>
            {t.quote && <div className="comment-quote">{t.quote}</div>}
            <div className="comment-text">{t.text}</div>
            {replies.get(t.id)?.map((r) => (
              <div key={r.id} className="reply">
                <div className="comment-meta">
                  <span className="avatar" style={{ background: avatarColor(r.author) }}>
                    {avatarText(r.author)}
                  </span>
                  <span className="who">{r.author}</span>
                  <span className="when">{formatDate(r.date)}</span>
                </div>
                <div className="comment-text">{r.text}</div>
              </div>
            ))}
          </div>
        ))}
      </div>
    </>
  );
});
