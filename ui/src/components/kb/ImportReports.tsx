import { AlertTriangle, CheckCircle2, MinusCircle, X, XCircle } from "lucide-react";
import type { KbImportReport } from "../../types";

interface Props {
  reports: KbImportReport[];
  onDismiss: () => void;
}

/** What happened to each file of the last import. */
export function ImportReports({ reports, onDismiss }: Props) {
  const imported = reports.filter((r) => !r.error && !r.unchanged).length;
  const failed = reports.filter((r) => r.error).length;
  return (
    <section className="card import-reports">
      <div className="section-head">
        <h3>导入结果</h3>
        <span className="muted">
          {imported} 个已导入{failed > 0 && `，${failed} 个失败`}
        </span>
        <span className="spacer" />
        <button type="button" className="icon-btn sm" title="关闭" onClick={onDismiss}>
          <X size={14} />
        </button>
      </div>
      {reports.map((r, i) => (
        <div key={`${r.fileName}-${i}`} className={`report${r.error ? " failed" : ""}`}>
          <div className="report-line">
            {r.error ? (
              <XCircle size={14} className="icon negative" />
            ) : r.unchanged ? (
              <MinusCircle size={14} className="icon" />
            ) : (
              <CheckCircle2 size={14} className="icon positive" />
            )}
            <span className="file">{r.fileName}</span>
            {r.parser && r.parser !== "builtin" && <span className="tag parser-tag">{r.parser === "mineru" ? "MinerU" : "PaddleOCR"}</span>}
            <span className="status">{r.error ? "导入失败" : r.unchanged ? "未变化，已跳过" : `${r.chunks} 个片段`}</span>
          </div>
          {r.error && <div className="report-note error">{r.error}</div>}
          {r.warnings.map((w) => (
            <div key={w} className="report-note warn">
              <AlertTriangle size={12} /> {w}
            </div>
          ))}
        </div>
      ))}
    </section>
  );
}
