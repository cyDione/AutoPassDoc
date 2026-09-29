import { useEffect, useState } from "react";
import { AlertCircle, GitMerge, Sparkles, Trash2 } from "lucide-react";
import type { Backend } from "../../api";
import type { Notify } from "../../hooks/useDocumentSession";
import { CASE_ACTION } from "../../labels";
import type { Case, Reviewer, ReviewerProfile } from "../../types";
import { avatarColor, avatarText, errorMessage, formatDay, percent } from "../../util";
import { ConfirmButton } from "../ConfirmButton";
import { PreReviewCard, type Section } from "./PreReviewCard";

interface Props {
  backend: Backend;
  reviewer: Reviewer;
  others: Reviewer[];
  /** Set when a document is open. */
  docId: number | null;
  section: Section | null;
  onUpdated: (reviewer: Reviewer) => void;
  onMerged: (into: number) => void;
  onDeleted: () => void;
  onJump: (paragraphIndex: number) => void;
  notify: Notify;
}

function thresholdText(threshold: number | null): string {
  return threshold === null ? "" : String(Math.round(threshold * 100));
}

export function ReviewerDetail({ backend, reviewer, others, docId, section, onUpdated, onMerged, onDeleted, onJump, notify }: Props) {
  const [name, setName] = useState(reviewer.name);
  const [note, setNote] = useState(reviewer.note);
  const [threshold, setThreshold] = useState(thresholdText(reviewer.threshold));
  const [saving, setSaving] = useState(false);
  const [formError, setFormError] = useState<string | null>(null);
  const [profile, setProfile] = useState<ReviewerProfile | null | undefined>(undefined);
  const [distilling, setDistilling] = useState(false);
  const [profileError, setProfileError] = useState<string | null>(null);
  const [cases, setCases] = useState<Case[] | null>(null);
  const [mergeInto, setMergeInto] = useState<number | "">("");

  useEffect(() => {
    backend
      .reviewerProfile(reviewer.id)
      .then(setProfile)
      .catch(() => setProfile(null));
    backend
      .reviewerCases(reviewer.id)
      .then(setCases)
      .catch(() => setCases([]));
  }, [backend, reviewer.id, reviewer.caseCount]);

  const dirty = name !== reviewer.name || note !== reviewer.note || threshold !== thresholdText(reviewer.threshold);

  const save = async () => {
    const t = threshold.trim();
    const value = t === "" ? null : Number(t) / 100;
    if (value !== null && (Number.isNaN(value) || value < 0.5 || value > 0.99)) {
      setFormError("个人阈值请填写 50–99 之间的数字，留空则使用全局阈值");
      return;
    }
    setSaving(true);
    setFormError(null);
    try {
      onUpdated(await backend.updateReviewer(reviewer.id, name, note, value));
    } catch (e) {
      setFormError(errorMessage(e));
    } finally {
      setSaving(false);
    }
  };

  const distill = async () => {
    setDistilling(true);
    setProfileError(null);
    try {
      setProfile(await backend.distillProfile(reviewer.id));
      notify(`已更新 ${reviewer.name} 的审稿画像`);
    } catch (e) {
      setProfileError(errorMessage(e));
    } finally {
      setDistilling(false);
    }
  };

  const merge = async () => {
    if (mergeInto === "") return;
    try {
      await backend.mergeReviewers(reviewer.id, mergeInto);
      notify(`已将 ${reviewer.name} 合并到 ${others.find((r) => r.id === mergeInto)?.name ?? "目标审稿人"}`);
      onMerged(mergeInto);
    } catch (e) {
      notify(`合并失败：${errorMessage(e)}`, true);
    }
  };

  const remove = async () => {
    try {
      await backend.deleteReviewer(reviewer.id);
      notify(`已删除审稿人 ${reviewer.name}`);
      onDeleted();
    } catch (e) {
      notify(`删除失败：${errorMessage(e)}`, true);
    }
  };

  return (
    <div className="reviewer-detail">
      <header className="page-head">
        <span className="avatar xl" style={{ background: avatarColor(reviewer.name) }}>
          {avatarText(reviewer.name)}
        </span>
        <div className="page-title">
          <h1>{reviewer.name}</h1>
          <div className="page-sub">
            {reviewer.caseCount} 条案例 · {reviewer.profileVersion ? `画像 v${reviewer.profileVersion}` : "暂无画像"} · 阈值{" "}
            {reviewer.threshold === null ? "跟随全局" : percent(reviewer.threshold)}
          </div>
        </div>
        <span className="spacer" />
        <ConfirmButton className="btn sm ghost" confirmLabel="确认删除？" onConfirm={() => void remove()}>
          <Trash2 size={13} /> 删除
        </ConfirmButton>
      </header>

      <section className="card">
        <div className="field-grid reviewer-form">
          <label className="field">
            <span className="field-label">姓名</span>
            <input className="input" value={name} onChange={(e) => setName(e.target.value)} />
          </label>
          <label className="field">
            <span className="field-label">个人阈值</span>
            <span className="input-suffix">
              <input
                className="input"
                inputMode="numeric"
                value={threshold}
                placeholder="跟随全局"
                onChange={(e) => setThreshold(e.target.value.replace(/[^\d.]/g, ""))}
              />
              <span>%</span>
            </span>
          </label>
          <label className="field wide">
            <span className="field-label">备注</span>
            <input className="input" value={note} placeholder="单位、职务、审稿侧重点等" onChange={(e) => setNote(e.target.value)} />
          </label>
        </div>
        <div className="form-actions">
          <button type="button" className="btn sm primary" disabled={!dirty || saving} onClick={() => void save()}>
            保存
          </button>
          {dirty && (
            <button
              type="button"
              className="btn sm ghost"
              onClick={() => {
                setName(reviewer.name);
                setNote(reviewer.note);
                setThreshold(thresholdText(reviewer.threshold));
                setFormError(null);
              }}
            >
              还原
            </button>
          )}
          {formError && <span className="form-message error">{formError}</span>}
        </div>
      </section>

      <section className="card profile-card">
        <div className="section-head">
          <h3>审稿画像</h3>
          {profile && (
            <span className="muted">
              v{profile.version} · {formatDay(profile.createdAt)} · 基于 {profile.caseCount} 条案例
            </span>
          )}
          <span className="spacer" />
          <button type="button" className="btn sm" disabled={distilling} onClick={() => void distill()}>
            {distilling ? <span className="spinner sm" /> : <Sparkles size={13} />}
            {distilling ? "正在提炼…" : "立即提炼画像"}
          </button>
        </div>
        {profileError && (
          <div className="friendly-error">
            <AlertCircle size={14} />
            <span>
              这次没能提炼画像：{profileError}
              {profile ? " 下面仍是上一版画像。" : ""}
            </span>
          </div>
        )}
        {profile === undefined && <div className="muted">正在读取…</div>}
        {profile === null && !profileError && (
          <p className="setting-hint">还没有画像。处理几条这位审稿人的批注后会自动提炼，也可以立即提炼。</p>
        )}
        {profile && (
          <>
            <p className="profile-summary">{profile.summary}</p>
            <div className="profile-lists">
              {(
                [
                  ["关注点", profile.focus],
                  ["表述偏好", profile.preferences],
                  ["常见要求", profile.commonRequests],
                ] as const
              ).map(([title, items]) => (
                <div key={title}>
                  <div className="list-title">{title}</div>
                  <ul>
                    {items.map((item) => (
                      <li key={item}>{item}</li>
                    ))}
                  </ul>
                </div>
              ))}
            </div>
          </>
        )}
      </section>

      {docId !== null && section && (
        <PreReviewCard backend={backend} docId={docId} reviewerId={reviewer.id} reviewerName={reviewer.name} section={section} onJump={onJump} />
      )}

      <section className="card">
        <div className="section-head">
          <h3>最近案例</h3>
          {cases && <span className="muted">{cases.length} 条</span>}
        </div>
        {cases?.length === 0 && <p className="setting-hint">还没有案例。应用或忽略这位审稿人批注的 AI 修复后，会记录在这里。</p>}
        <div className="case-list">
          {cases?.slice(0, 30).map((c) => {
            const action = CASE_ACTION[c.action];
            return (
              <div key={c.id} className="case-item">
                <div className="case-comment">{c.comment}</div>
                <div className="case-meta">
                  <span className={`pill ${action.tone}`}>{action.label}</span>
                  {c.confidence !== null && <span className="muted">置信度 {percent(c.confidence)}</span>}
                  {c.category && <span className="tag">{c.category}</span>}
                  <span className="spacer" />
                  <span className="muted" title={c.docName}>
                    {formatDay(c.createdAt)}
                  </span>
                </div>
              </div>
            );
          })}
        </div>
      </section>

      {others.length > 0 && (
        <section className="card merge-card">
          <GitMerge size={15} className="icon" />
          <span>合并到…</span>
          <select className="select" value={mergeInto} onChange={(e) => setMergeInto(e.target.value ? Number(e.target.value) : "")}>
            <option value="">选择审稿人</option>
            {others.map((r) => (
              <option key={r.id} value={r.id}>
                {r.name}
              </option>
            ))}
          </select>
          <ConfirmButton className="btn sm" disabled={mergeInto === ""} confirmLabel="确认合并？" onConfirm={() => void merge()}>
            合并
          </ConfirmButton>
          <span className="muted small">署名映射、案例都会并入对方，此审稿人随后删除。</span>
        </section>
      )}
    </div>
  );
}
