import { useState } from "react";
import { FileDown } from "lucide-react";
import type { Backend } from "../../api";
import { errorMessage } from "../../util";

export function DataTab({ backend }: { backend: Backend }) {
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<{ text: string; error?: boolean } | null>(null);

  const exportDataset = async () => {
    setBusy(true);
    setResult(null);
    try {
      const path = await backend.exportDataset();
      if (path) setResult({ text: `已导出到 ${path}` });
    } catch (e) {
      setResult({ text: `导出失败：${errorMessage(e)}`, error: true });
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="settings-section">
      <div className="section-head">
        <h3>数据</h3>
      </div>
      <div className="card data-card">
        <div className="setting-label">评判训练数据</div>
        <p className="setting-hint">
          把每次“采纳 / 修改后采纳 / 拒绝”的批注、原文、AI 修改和最终文本导出为 JSONL，用于校准或微调 Laya / Jev 评判模型，让置信度更贴近你自己的判断。
        </p>
        <div className="form-actions">
          <button type="button" className="btn sm" disabled={busy} onClick={() => void exportDataset()}>
            {busy ? <span className="spinner sm" /> : <FileDown size={13} />}导出评判训练数据（JSONL）
          </button>
          {result && <span className={`form-message${result.error ? " error" : ""}`}>{result.text}</span>}
        </div>
      </div>
    </div>
  );
}
