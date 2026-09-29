import { useEffect, useMemo, useState } from "react";
import { Check, Plus, UserX } from "lucide-react";
import type { Backend } from "../api";
import type { Notify, RunEdit } from "../hooks/useDocumentSession";
import type { AuthorView, Reviewer } from "../types";
import { avatarColor, avatarText, errorMessage } from "../util";
import { Floating } from "./Floating";

interface Props {
  backend: Backend;
  docId: number;
  author: AuthorView;
  anchor: HTMLElement;
  edit: RunEdit;
  onAuthors: (authors: AuthorView[]) => void;
  onClose: () => void;
  notify: Notify;
}

/** Maps a comment signature to a reviewer: pick one, create one or clear the mapping. */
export function AuthorPopover({ backend, docId, author, anchor, edit, onAuthors, onClose, notify }: Props) {
  const [reviewers, setReviewers] = useState<Reviewer[] | null>(null);
  const [query, setQuery] = useState("");
  const [writeBack, setWriteBack] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    backend
      .reviewers()
      .then(setReviewers)
      .catch((e) => setError(errorMessage(e)));
  }, [backend]);

  const trimmed = query.trim();
  const matches = useMemo(
    () => (reviewers ?? []).filter((r) => !trimmed || r.name.includes(trimmed) || r.note.includes(trimmed)),
    [reviewers, trimmed],
  );
  const canCreate = trimmed !== "" && !(reviewers ?? []).some((r) => r.name === trimmed);

  const assign = async (reviewerId: number | null, newReviewer: string | null) => {
    setBusy(true);
    setError(null);
    try {
      const outcome = await edit(async () => {
        const result = await backend.assignAuthor(docId, author.author, author.initials, reviewerId, newReviewer, writeBack);
        onAuthors(result.authors);
        return result.outcome;
      });
      const name = newReviewer ?? reviewers?.find((r) => r.id === reviewerId)?.name;
      notify(
        name
          ? `已将“${author.author}”归到 ${name}${outcome ? "，并修改了文档中的署名" : ""}`
          : `已取消“${author.author}”的关联`,
      );
      onClose();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    // The reviewer list loads after the popover opens; plan for its full height when picking a side.
    <Floating anchor={anchor} onClose={onClose} width={296} expectedHeight={420} className="popover author-popover">
      <div className="popover-head">
        <div className="title">
          批注署名「{author.author}」<span className="muted">{author.commentCount} 条</span>
        </div>
        <div className="sub">{author.reviewer ? `已归到审稿人 ${author.reviewer.name}` : "还没有对应的审稿人"}</div>
        {author.generic && <div className="note">通用署名，仅在本文档生效</div>}
      </div>
      <input
        className="input"
        autoFocus
        placeholder="搜索或输入新审稿人姓名"
        value={query}
        disabled={busy}
        onChange={(e) => setQuery(e.target.value)}
        onKeyDown={(e) => {
          if (e.key !== "Enter" || busy) return;
          if (matches.length === 1 && !canCreate) void assign(matches[0].id, null);
          else if (canCreate) void assign(null, trimmed);
        }}
      />
      <div className="popover-list">
        {reviewers === null && !error && <div className="popover-empty">正在加载…</div>}
        {matches.map((r) => {
          const current = author.reviewer?.id === r.id;
          return (
            <button
              key={r.id}
              type="button"
              className={`popover-item${current ? " on" : ""}`}
              disabled={busy}
              onClick={() => void assign(r.id, null)}
            >
              <span className="avatar" style={{ background: avatarColor(r.name) }}>
                {avatarText(r.name)}
              </span>
              <span className="name">{r.name}</span>
              <span className="muted">{r.caseCount} 条案例</span>
              {current && <Check size={14} className="check-icon" />}
            </button>
          );
        })}
        {canCreate && (
          <button type="button" className="popover-item create" disabled={busy} onClick={() => void assign(null, trimmed)}>
            <Plus size={14} /> 新建审稿人「{trimmed}」
          </button>
        )}
        {reviewers !== null && matches.length === 0 && !canCreate && <div className="popover-empty">还没有审稿人，输入姓名即可新建</div>}
      </div>
      {error && <div className="popover-error">{error}</div>}
      <div className="popover-foot">
        <label className="check">
          <input type="checkbox" checked={writeBack} disabled={busy} onChange={(e) => setWriteBack(e.target.checked)} />
          同时修改文档中的批注署名
        </label>
        {author.reviewer && (
          <button type="button" className="link-btn danger" disabled={busy} onClick={() => void assign(null, null)}>
            <UserX size={13} /> 取消关联
          </button>
        )}
      </div>
    </Floating>
  );
}
