import { memo, useCallback, useEffect, useState } from "react";
import { Plus, Users } from "lucide-react";
import type { Backend } from "../api";
import type { Section } from "../components/reviewers/PreReviewCard";
import { ReviewerDetail } from "../components/reviewers/ReviewerDetail";
import type { Notify } from "../hooks/useDocumentSession";
import type { Reviewer } from "../types";
import { avatarColor, avatarText, errorMessage } from "../util";

interface Props {
  backend: Backend;
  /** The page is on screen; data is refreshed each time it becomes visible. */
  active: boolean;
  docId: number | null;
  section: Section | null;
  onJump: (paragraphIndex: number) => void;
  notify: Notify;
}

export const ReviewersPage = memo(function ReviewersPage({ backend, active, docId, section, onJump, notify }: Props) {
  const [reviewers, setReviewers] = useState<Reviewer[] | null>(null);
  const [selected, setSelected] = useState<number | null>(null);
  const [newName, setNewName] = useState<string | null>(null);

  const load = useCallback(
    (select?: number) =>
      backend
        .reviewers()
        .then((list) => {
          setReviewers(list);
          setSelected((current) => {
            const wanted = select ?? current;
            return list.some((r) => r.id === wanted) ? wanted : (list[0]?.id ?? null);
          });
        })
        .catch((e) => notify(`无法读取审稿人：${errorMessage(e)}`, true)),
    [backend, notify],
  );

  useEffect(() => {
    if (active) void load();
  }, [active, load]);

  const create = async () => {
    const name = newName?.trim();
    if (!name) return setNewName(null);
    try {
      const r = await backend.createReviewer(name, "");
      setNewName(null);
      void load(r.id);
    } catch (e) {
      notify(errorMessage(e), true);
    }
  };

  const current = reviewers?.find((r) => r.id === selected) ?? null;

  if (reviewers !== null && reviewers.length === 0 && newName === null) {
    return (
      <div className="page scroll">
        <div className="empty-state">
          <Users size={34} strokeWidth={1.4} />
          <h2>还没有审稿人</h2>
          <p>审稿人会在你给批注作者归类后出现。在批注卡片上点击作者名，就能把“Administrator”这类署名归到真实的专家名下。</p>
          <button type="button" className="btn" onClick={() => setNewName("")}>
            <Plus size={15} /> 新建审稿人
          </button>
        </div>
      </div>
    );
  }

  return (
    <div className="reviewers-layout">
      <aside className="reviewer-list">
        <div className="list-head">
          <span>审稿人</span>
          {reviewers && <span className="muted">{reviewers.length}</span>}
          <span className="spacer" />
          <button type="button" className="icon-btn sm" title="新建审稿人" onClick={() => setNewName("")}>
            <Plus size={15} />
          </button>
        </div>
        {newName !== null && (
          <form
            className="new-reviewer"
            onSubmit={(e) => {
              e.preventDefault();
              void create();
            }}
          >
            <input
              className="input"
              autoFocus
              value={newName}
              placeholder="姓名，回车确认"
              onChange={(e) => setNewName(e.target.value)}
              onBlur={() => !newName.trim() && setNewName(null)}
              onKeyDown={(e) => e.key === "Escape" && setNewName(null)}
            />
          </form>
        )}
        <div className="scroll list-body">
          {reviewers?.map((r) => (
            <button key={r.id} type="button" className={`reviewer-item${r.id === selected ? " on" : ""}`} onClick={() => setSelected(r.id)}>
              <span className="avatar lg" style={{ background: avatarColor(r.name) }}>
                {avatarText(r.name)}
              </span>
              <span className="main">
                <span className="name">{r.name}</span>
                <span className="meta">
                  {r.caseCount} 条案例 · {r.profileVersion ? `画像 v${r.profileVersion}` : "暂无画像"}
                </span>
              </span>
            </button>
          ))}
        </div>
      </aside>
      <div className="reviewer-pane scroll">
        {current && (
          <ReviewerDetail
            key={current.id}
            backend={backend}
            reviewer={current}
            others={reviewers!.filter((r) => r.id !== current.id)}
            docId={docId}
            section={section}
            onUpdated={(r) => setReviewers((list) => list?.map((x) => (x.id === r.id ? r : x)) ?? null)}
            onMerged={(into) => void load(into)}
            onDeleted={() => void load()}
            onJump={onJump}
            notify={notify}
          />
        )}
      </div>
    </div>
  );
});
