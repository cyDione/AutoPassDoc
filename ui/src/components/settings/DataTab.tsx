import { useEffect, useState } from "react";
import { ArchiveRestore, FileDown, HardDriveDownload } from "lucide-react";
import type { Backend } from "../../api";
import type { BackupImportMode, BackupManifest, BackupProgress } from "../../types";
import { errorMessage } from "../../util";

type Message = { text: string; error?: boolean } | null;

function manifestText(m: BackupManifest): string {
  return `${m.documents} 份资料、${m.chunks} 个分块、${m.reviewers} 位审稿人、${m.cases} 条案例`;
}

function sizeText(bytes: number): string {
  return bytes >= 1_000_000 ? `${(bytes / 1_048_576).toFixed(1)} MB` : `${Math.max(1, Math.round(bytes / 1024))} KB`;
}

/** Imports a backup: pick the file, see what it holds, choose merge or replace. */
function RestoreFlow({ backend, busy, setBusy }: { backend: Backend; busy: boolean; setBusy: (b: boolean) => void }) {
  const [picked, setPicked] = useState<{ path: string; manifest: BackupManifest } | null>(null);
  const [mode, setMode] = useState<BackupImportMode>("merge");
  const [message, setMessage] = useState<Message>(null);

  const pick = async () => {
    setMessage(null);
    try {
      const path = await backend.pickBackup();
      if (!path) return;
      setPicked({ path, manifest: await backend.inspectBackup(path) });
    } catch (e) {
      setMessage({ text: errorMessage(e), error: true });
    }
  };

  const restore = async () => {
    if (!picked) return;
    if (mode === "replace" && !(await backend.confirm("当前的知识库和审稿人数据会被备份文件替换。替换前会自动备份当前数据。", "替换全部数据", "替换"))) return;
    setBusy(true);
    setMessage(null);
    try {
      const r = await backend.importBackup(picked.path, mode);
      const parts =
        r.mode === "replace"
          ? [`已替换为备份中的数据：${manifestText(r.manifest)}`, r.autoBackup ? `原数据已备份到 ${r.autoBackup}` : ""]
          : [
              `新增 ${r.documentsAdded} 份资料、${r.reviewersAdded} 位审稿人、${r.casesAdded} 条案例`,
              r.documentsSkipped + r.reviewersSkipped + r.casesSkipped > 0
                ? `跳过已有的 ${r.documentsSkipped} 份资料、${r.reviewersSkipped} 位审稿人、${r.casesSkipped} 条案例`
                : "",
            ];
      setMessage({ text: parts.filter(Boolean).join("；") });
      setPicked(null);
    } catch (e) {
      setMessage({ text: `导入失败：${errorMessage(e)}`, error: true });
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      {picked ? (
        <div className="restore-box">
          <div className="mono ellipsis" title={picked.path}>
            {picked.path}
          </div>
          <div className="muted">
            {new Date(picked.manifest.createdAt * 1000).toLocaleString("zh-CN")} 由 {picked.manifest.appVersion} 版创建，包含 {manifestText(picked.manifest)}
          </div>
          <div className="radio-group vertical">
            <label className="radio with-hint">
              <input type="radio" checked={mode === "merge"} onChange={() => setMode("merge")} />
              <span>
                合并（推荐）
                <small>只添加当前没有的资料、审稿人和案例，同名审稿人沿用现有的</small>
              </span>
            </label>
            <label className="radio with-hint">
              <input type="radio" checked={mode === "replace"} onChange={() => setMode("replace")} />
              <span>
                替换
                <small>用备份替换当前全部知识库和审稿人数据，替换前自动备份当前数据</small>
              </span>
            </label>
          </div>
          <div className="form-actions">
            <button type="button" className={`btn sm ${mode === "replace" ? "danger" : "primary"}`} disabled={busy} onClick={() => void restore()}>
              {mode === "replace" ? "替换全部数据" : "合并导入"}
            </button>
            <button type="button" className="btn sm ghost" disabled={busy} onClick={() => setPicked(null)}>
              取消
            </button>
          </div>
        </div>
      ) : (
        <div className="form-actions">
          <button type="button" className="btn sm" disabled={busy} onClick={() => void pick()}>
            <ArchiveRestore size={13} /> 从备份导入…
          </button>
        </div>
      )}
      {message && <div className={`form-message${message.error ? " error" : ""}`}>{message.text}</div>}
    </>
  );
}

export function DataTab({ backend }: { backend: Backend }) {
  const [busy, setBusy] = useState(false);
  const [progress, setProgress] = useState<BackupProgress | null>(null);
  const [backupMessage, setBackupMessage] = useState<Message>(null);
  const [result, setResult] = useState<Message>(null);

  useEffect(() => backend.onBackupProgress(setProgress), [backend]);
  useEffect(() => {
    if (!busy) setProgress(null);
  }, [busy]);

  const exportBackup = async () => {
    setBusy(true);
    setBackupMessage(null);
    try {
      const r = await backend.exportBackup();
      if (r) {
        const missing = r.missingFiles.length > 0 ? `；${r.missingFiles.length} 份资料的原文件已不在磁盘上，未打包` : "";
        setBackupMessage({ text: `已备份 ${manifestText(r.manifest)}（${sizeText(r.size)}）到 ${r.path}${missing}` });
      }
    } catch (e) {
      setBackupMessage({ text: `备份失败：${errorMessage(e)}`, error: true });
    } finally {
      setBusy(false);
    }
  };

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
        <div className="setting-label">备份与迁移</div>
        <p className="setting-hint">
          把全部知识库（资料原文件、分块和向量）和审稿人（署名对应、案例、画像）打包成一个 .apdbak 文件，用于换电脑或定期备份。备份不含 API
          Key，导入后需要重新填写。
        </p>
        <div className="form-actions">
          <button type="button" className="btn sm primary" disabled={busy} onClick={() => void exportBackup()}>
            <HardDriveDownload size={13} /> 备份全部数据…
          </button>
        </div>
        {backupMessage && <div className={`form-message${backupMessage.error ? " error" : ""}`}>{backupMessage.text}</div>}
        <RestoreFlow backend={backend} busy={busy} setBusy={setBusy} />
        {busy && progress && progress.total > 0 && (
          <div className="backup-progress">
            <div className="bar">
              <i style={{ width: `${Math.round((progress.done / progress.total) * 100)}%` }} />
            </div>
            <span className="muted">{progress.step}</span>
          </div>
        )}
      </div>
      <div className="card data-card">
        <div className="setting-label">评判训练数据</div>
        <p className="setting-hint">
          把每次“采纳 / 修改后采纳 / 拒绝”的批注、原文、AI 修改和最终文本导出为 JSONL，用于校准或微调 Laya / Jev 评判模型，让置信度更贴近你自己的判断。
        </p>
        <div className="form-actions">
          <button type="button" className="btn sm" disabled={busy} onClick={() => void exportDataset()}>
            <FileDown size={13} /> 导出评判训练数据（JSONL）
          </button>
          {result && <span className={`form-message${result.error ? " error" : ""}`}>{result.text}</span>}
        </div>
      </div>
    </div>
  );
}
