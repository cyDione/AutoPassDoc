import { useEffect, useMemo, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { ChevronDown, ChevronUp, ExternalLink, Search, X } from "lucide-react";
import type { Backend } from "../../api";
import type { KbDocument, KbDocumentView } from "../../types";
import { errorMessage, formatDay, formatNumber } from "../../util";

interface Props {
  backend: Backend;
  doc: KbDocument;
  onOpen: (path: string) => void;
  onClose: () => void;
}

const PARSER_LABEL: Record<string, string> = { builtin: "普通解析", mineru: "MinerU 增强解析", paddleocr: "PaddleOCR 增强解析" };

interface Row {
  text: string;
  headingLevel: number | null;
  /** Chunks that start on this line. */
  chunks: { n: number; headingPath: string[] }[];
}

function rowsOf(view: KbDocumentView): Row[] {
  const rows: Row[] = view.lines.map((l) => ({ ...l, chunks: [] }));
  // Line i starts after the i newlines before it.
  const starts: number[] = [];
  let offset = 0;
  for (const l of view.lines) {
    starts.push(offset);
    offset += [...l.text].length + 1;
  }
  let line = 0;
  view.chunks.forEach((c, n) => {
    while (line + 1 < starts.length && starts[line + 1] <= c.charStart) line++;
    rows[line]?.chunks.push({ n: n + 1, headingPath: c.headingPath });
  });
  return rows;
}

function highlight(text: string, q: string, current: boolean) {
  if (!q) return text;
  const parts = text.split(q);
  return parts.flatMap((p, i) =>
    i === 0
      ? [p]
      : [
          <mark key={i} className={current ? "current" : undefined}>
            {q}
          </mark>,
          p,
        ],
  );
}

/** Shows what the knowledge base holds for one document: text, headings and chunks. */
export function KbViewer({ backend, doc, onOpen, onClose }: Props) {
  const [view, setView] = useState<KbDocumentView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [showChunks, setShowChunks] = useState(false);
  const [query, setQuery] = useState("");
  const [hit, setHit] = useState(0);
  const bodyRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    let cancelled = false;
    backend
      .kbDocumentView(doc.id)
      .then((v) => !cancelled && setView(v))
      .catch((e) => !cancelled && setError(errorMessage(e)));
    return () => {
      cancelled = true;
    };
  }, [backend, doc.id]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const rows = useMemo(() => (view ? rowsOf(view) : []), [view]);
  const q = query.trim();
  const hits = useMemo(() => (q ? rows.flatMap((r, i) => (r.text.includes(q) ? [i] : [])) : []), [rows, q]);

  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => bodyRef.current,
    estimateSize: (i) => (rows[i]?.headingLevel != null ? 40 : 28) + (showChunks ? rows[i]?.chunks.length * 24 : 0),
    overscan: 20,
  });

  // Jump to the first match as the query changes.
  const first = hits[0];
  useEffect(() => {
    if (first !== undefined) virtualizer.scrollToIndex(first, { align: "center" });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [first, q]);

  const go = (i: number) => {
    if (hits.length === 0) return;
    const next = (i + hits.length) % hits.length;
    setHit(next);
    virtualizer.scrollToIndex(hits[next], { align: "center" });
  };

  const d = view?.document ?? doc;
  const meta = [d.meta.docNumber, d.meta.issuer, d.meta.date].filter(Boolean).join(" · ");

  return (
    <div className="dialog-backdrop" role="presentation" onClick={onClose}>
      <div className="dialog kb-viewer" role="dialog" aria-modal aria-label={`查看 ${d.title}`} onClick={(e) => e.stopPropagation()}>
        <div className="dialog-head">
          <div className="viewer-title">
            <h2 className="ellipsis" title={d.title}>
              {d.title}
            </h2>
            <div className="muted ellipsis">
              {meta && `${meta} · `}
              {d.fileName} · {PARSER_LABEL[d.parser] ?? d.parser} · {formatNumber(d.charCount)} 字 · {formatNumber(d.chunkCount)} 个片段 · 导入于{" "}
              {formatDay(d.importedAt)}
            </div>
          </div>
          <span className="spacer" />
          <button type="button" className="icon-btn" title="关闭" onClick={onClose}>
            <X size={17} />
          </button>
        </div>
        <div className="viewer-bar">
          <div className="input-icon small">
            <Search size={14} />
            <input
              className="input"
              value={query}
              placeholder="在全文中查找"
              onChange={(e) => {
                setQuery(e.target.value);
                setHit(0);
              }}
              onKeyDown={(e) => e.key === "Enter" && go(e.shiftKey ? hit - 1 : hit + 1)}
            />
          </div>
          {q && (
            <>
              <span className="muted">{hits.length ? `${hit + 1} / ${hits.length} 行` : "无结果"}</span>
              <button type="button" className="icon-btn sm" title="上一个" disabled={!hits.length} onClick={() => go(hit - 1)}>
                <ChevronUp size={14} />
              </button>
              <button type="button" className="icon-btn sm" title="下一个" disabled={!hits.length} onClick={() => go(hit + 1)}>
                <ChevronDown size={14} />
              </button>
            </>
          )}
          <span className="spacer" />
          <label className="check">
            <input type="checkbox" checked={showChunks} onChange={(e) => setShowChunks(e.target.checked)} /> 显示分块
          </label>
          <button type="button" className="btn sm" onClick={() => onOpen(d.storedPath)}>
            <ExternalLink size={13} /> 打开原文件
          </button>
        </div>
        {d.warnings.length > 0 && (
          <div className="viewer-warnings">
            {d.warnings.map((w) => (
              <div key={w} className="fix-note warn">
                {w}
              </div>
            ))}
          </div>
        )}
        <div className="viewer-body scroll" ref={bodyRef}>
          {error && <div className="fix-note error">{error}</div>}
          {!view && !error && (
            <div className="loading">
              <span className="spinner" /> 正在读取…
            </div>
          )}
          <div style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
            {virtualizer.getVirtualItems().map((item) => {
              const r = rows[item.index];
              return (
                <div key={item.key} data-index={item.index} ref={virtualizer.measureElement} className="viewer-row" style={{ transform: `translateY(${item.start}px)` }}>
                  {showChunks &&
                    r.chunks.map((c) => (
                      <div key={c.n} className="chunk-mark">
                        <span>片段 {c.n}</span>
                        {c.headingPath.length > 0 && <span className="muted ellipsis">{c.headingPath.join(" › ")}</span>}
                      </div>
                    ))}
                  <div className={r.headingLevel != null ? `viewer-line heading h${Math.min(r.headingLevel, 9)}` : "viewer-line"}>
                    {r.text ? highlight(r.text, q, hits[hit] === item.index) : " "}
                  </div>
                </div>
              );
            })}
          </div>
        </div>
      </div>
    </div>
  );
}
