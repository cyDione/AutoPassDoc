import { useState } from "react";
import { ScanSearch } from "lucide-react";
import type { Backend } from "../../api";
import type { PreReviewItem } from "../../types";
import { errorMessage } from "../../util";

/** The outline section the document view is showing: paragraphs start..end (exclusive). */
export interface Section {
  start: number;
  end: number;
  title: string;
}

interface Props {
  backend: Backend;
  docId: number;
  reviewerId: number;
  reviewerName: string;
  section: Section;
  onJump: (paragraphIndex: number) => void;
}

/** 按此审稿人预审: comments this reviewer would likely make on the current section. */
export function PreReviewCard({ backend, docId, reviewerId, reviewerName, section, onJump }: Props) {
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<{ section: Section; items: PreReviewItem[] } | null>(null);
  const [error, setError] = useState<string | null>(null);

  const run = async () => {
    setBusy(true);
    setError(null);
    try {
      setResult({ section, items: await backend.preReview(docId, reviewerId, section.start, section.end) });
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="card">
      <div className="section-head">
        <h3>按此审稿人预审</h3>
        <span className="spacer" />
        <button type="button" className="btn sm" disabled={busy} onClick={() => void run()}>
          {busy ? <span className="spinner sm" /> : <ScanSearch size={13} />}
          {busy ? "正在预审…" : "预审当前章节"}
        </button>
      </div>
      <p className="setting-hint">
        模拟 {reviewerName} 的视角，先把当前章节「{section.title}」（第 {section.start + 1}–{section.end} 段）扫一遍，提前找出他大概率会批注的地方。
      </p>
      {error && <div className="form-message error">{error}</div>}
      {result && (
        <div className="pre-review">
          <div className="muted small">
            「{result.section.title}」{result.items.length ? `预计 ${result.items.length} 条批注，点击定位到原文` : "中没有发现明显问题"}
          </div>
          {result.items.map((item, i) => (
            <button key={`${item.paragraphIndex}-${i}`} type="button" className="pre-item" onClick={() => onJump(item.paragraphIndex)}>
              <span className="pre-quote">{item.quote}</span>
              <span className="pre-comment">{item.comment}</span>
              <span className="pre-meta">
                {item.category && <span className="tag">{item.category}</span>}
                <span className="muted">第 {item.paragraphIndex + 1} 段</span>
              </span>
            </button>
          ))}
        </div>
      )}
    </section>
  );
}
