import { memo, useMemo, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { AlertTriangle, ExternalLink, Pencil, Search, Trash2 } from "lucide-react";
import type { KbDocument, KbMeta } from "../../types";
import { errorMessage, formatDay, formatNumber } from "../../util";
import { ConfirmButton } from "../ConfirmButton";

const ROW_HEIGHT = 52;


interface Props {
  documents: KbDocument[];
  onOpen: (path: string) => void;
  onRemove: (doc: KbDocument) => Promise<void>;
  onUpdateMeta: (doc: KbDocument, meta: KbMeta) => Promise<void>;
}

/** All knowledge-base documents; virtualized so thousands of rows stay fast. */
export const KbDocumentTable = memo(function KbDocumentTable({ documents, onOpen, onRemove, onUpdateMeta }: Props) {
  const [filter, setFilter] = useState("");
  const [editing, setEditing] = useState<number | null>(null);
  const bodyRef = useRef<HTMLDivElement>(null);

  const rows = useMemo(() => {
    const q = filter.trim().toLowerCase();
    if (!q) return documents;
    return documents.filter((d) =>
      [d.title, d.fileName, d.meta.docNumber, d.meta.issuer, d.meta.date].some((v) => v?.toLowerCase().includes(q)),
    );
  }, [documents, filter]);

  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => bodyRef.current,
    estimateSize: (i) => (rows[i]?.id === editing ? 190 : ROW_HEIGHT),
    getItemKey: (i) => rows[i].id,
    overscan: 10,
  });

  return (
    <section className="card kb-table">
      <div className="section-head">
        <h3>资料</h3>
        <span className="muted">{filter.trim() ? `${rows.length} / ${documents.length} 份` : `${documents.length} 份`}</span>
        <span className="spacer" />
        <div className="input-icon small">
          <Search size={14} />
          <input className="input" value={filter} placeholder="筛选标题、文号、发文机关" onChange={(e) => setFilter(e.target.value)} />
        </div>
      </div>
      <div className="kb-row kb-row-head">
        <span>标题</span>
        <span>文号</span>
        <span className="col-issuer">发文机关</span>
        <span>日期</span>
        <span className="num">片段</span>
        <span className="col-imported">导入时间</span>
        <span />
      </div>
      <div className="kb-table-body scroll" ref={bodyRef}>
        {rows.length === 0 && <div className="panel-empty">没有符合条件的资料</div>}
        <div style={{ height: virtualizer.getTotalSize(), position: "relative" }}>
          {virtualizer.getVirtualItems().map((item) => {
            const d = rows[item.index];
            const warnings = d.warnings ?? [];
            return (
              <div
                key={item.key}
                data-index={item.index}
                ref={virtualizer.measureElement}
                className="kb-item"
                style={{ transform: `translateY(${item.start}px)` }}
              >
                <div className="kb-row">
                  <span className="title-cell">
                    <span className="title-line">
                      {warnings.length > 0 && (
                        <span className="import-warning" title={warnings.join("\n")}>
                          <AlertTriangle size={13} />
                        </span>
                      )}
                      <span className="title" title={d.title}>
                        {d.title}
                      </span>
                    </span>
                    <span className="file" title={d.originalPath}>
                      {d.fileName}
                    </span>
                  </span>
                  <span className="ellipsis" title={d.meta.docNumber ?? undefined}>
                    {d.meta.docNumber || <span className="muted">—</span>}
                  </span>
                  <span className="ellipsis col-issuer" title={d.meta.issuer ?? undefined}>
                    {d.meta.issuer || <span className="muted">—</span>}
                  </span>
                  <span>{d.meta.date || <span className="muted">—</span>}</span>
                  <span className="num">{formatNumber(d.chunkCount)}</span>
                  <span className="muted col-imported">{formatDay(d.importedAt)}</span>
                  <span className="row-actions">
                    <button type="button" className="icon-btn sm" title="编辑元数据" onClick={() => setEditing(editing === d.id ? null : d.id)}>
                      <Pencil size={14} />
                    </button>
                    <button type="button" className="icon-btn sm" title="用默认程序打开" onClick={() => onOpen(d.storedPath)}>
                      <ExternalLink size={14} />
                    </button>
                    <ConfirmButton className="icon-btn sm" title="从知识库删除" confirmLabel="确认删除" onConfirm={() => void onRemove(d)}>
                      <Trash2 size={14} />
                    </ConfirmButton>
                  </span>
                </div>
                {editing === d.id && (
                  <MetaForm
                    doc={d}
                    onCancel={() => setEditing(null)}
                    onSave={async (meta) => {
                      await onUpdateMeta(d, meta);
                      setEditing(null);
                    }}
                  />
                )}
              </div>
            );
          })}
        </div>
      </div>
    </section>
  );
});

interface FormProps {
  doc: KbDocument;
  onSave: (meta: KbMeta) => Promise<void>;
  onCancel: () => void;
}

function MetaForm({ doc, onSave, onCancel }: FormProps) {
  const [title, setTitle] = useState(doc.meta.title ?? doc.title);
  const [docNumber, setDocNumber] = useState(doc.meta.docNumber ?? "");
  const [issuer, setIssuer] = useState(doc.meta.issuer ?? "");
  const [date, setDate] = useState(doc.meta.date ?? "");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const orNull = (v: string) => v.trim() || null;

  return (
    <form
      className="meta-form"
      onSubmit={async (e) => {
        e.preventDefault();
        setBusy(true);
        setError(null);
        try {
          await onSave({ title: orNull(title), docNumber: orNull(docNumber), issuer: orNull(issuer), date: orNull(date) });
        } catch (err) {
          setError(errorMessage(err));
          setBusy(false);
        }
      }}
    >
      <div className="field-grid four">
        <label className="field wide">
          <span className="field-label">标题</span>
          <input className="input" value={title} onChange={(e) => setTitle(e.target.value)} />
        </label>
        <label className="field">
          <span className="field-label">文号</span>
          <input className="input" value={docNumber} placeholder="例如：国办发〔2024〕12号" onChange={(e) => setDocNumber(e.target.value)} />
        </label>
        <label className="field">
          <span className="field-label">发文机关</span>
          <input className="input" value={issuer} onChange={(e) => setIssuer(e.target.value)} />
        </label>
        <label className="field">
          <span className="field-label">日期</span>
          <input className="input" value={date} placeholder="2024-03-15" onChange={(e) => setDate(e.target.value)} />
        </label>
      </div>
      <div className="form-actions">
        <button type="submit" className="btn sm primary" disabled={busy}>
          保存
        </button>
        <button type="button" className="btn sm ghost" disabled={busy} onClick={onCancel}>
          取消
        </button>
        {error && <span className="form-message error">{error}</span>}
      </div>
    </form>
  );
}
