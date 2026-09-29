import { memo, useCallback, useEffect, useState } from "react";
import { AlertTriangle, FilePlus2, Library, UploadCloud, X, Zap } from "lucide-react";
import type { Backend } from "../api";
import { ImportReports } from "../components/kb/ImportReports";
import { KbDocumentTable } from "../components/kb/KbDocumentTable";
import { KbSearch } from "../components/kb/KbSearch";
import type { Notify } from "../hooks/useDocumentSession";
import type { KbDocument, KbImportReport, KbMeta, KbProgress, KbStats, Settings } from "../types";
import { errorMessage, formatNumber } from "../util";

export interface DroppedFiles {
  paths: string[];
  /** Distinguishes two drops of the same files. */
  nonce: number;
}

interface Props {
  backend: Backend;
  /** The page is on screen; data is refreshed each time it becomes visible. */
  active: boolean;
  settings: Settings | null;
  dropped: DroppedFiles | null;
  notify: Notify;
}

const STAGE: Record<KbProgress["stage"], { running: string; failed: string }> = {
  import: { running: "正在导入", failed: "导入失败" },
  embed: { running: "正在向量化", failed: "向量化失败" },
};

function statsLine(stats: KbStats): string {
  const model = stats.embeddingModel ? `（${stats.embeddingModel.split("/").pop()}）` : "（未配置向量模型）";
  return `${formatNumber(stats.documents)} 份资料 · ${formatNumber(stats.chunks)} 个片段 · 已向量化 ${formatNumber(stats.embedded)} / ${formatNumber(stats.chunks)}${model}`;
}

function DropZone({ onPick, busy }: { onPick: () => void; busy: boolean }) {
  return (
    <button type="button" className="drop-zone" disabled={busy} onClick={onPick}>
      <UploadCloud size={20} strokeWidth={1.6} />
      <span>拖入 Word、PDF、TXT、Markdown 文件导入</span>
      <span className="muted">或点击选择文件</span>
    </button>
  );
}

export const KnowledgeBasePage = memo(function KnowledgeBasePage({ backend, active, settings, dropped, notify }: Props) {
  const [documents, setDocuments] = useState<KbDocument[] | null>(null);
  const [stats, setStats] = useState<KbStats | null>(null);
  const [progress, setProgress] = useState<KbProgress | null>(null);
  /** A stage that stopped with an error; the backend reports it as progress with `total` 0 and a message. */
  const [failure, setFailure] = useState<{ stage: KbProgress["stage"]; message: string } | null>(null);
  const [reports, setReports] = useState<KbImportReport[]>([]);
  const [importing, setImporting] = useState(false);

  const refresh = useCallback(() => {
    Promise.all([backend.kbDocuments(), backend.kbStats()])
      .then(([docs, s]) => {
        setDocuments(docs);
        setStats(s);
      })
      .catch((e) => notify(`无法读取知识库：${errorMessage(e)}`, true));
  }, [backend, notify]);

  useEffect(() => {
    if (active) refresh();
  }, [active, refresh]);

  useEffect(() => {
    let clear = 0;
    const off = backend.onKbProgress((p) => {
      clearTimeout(clear);
      if (p.total === 0) {
        // Nothing to do, or the stage failed.
        setProgress(null);
        if (p.message) setFailure({ stage: p.stage, message: p.message });
        refresh();
        return;
      }
      setFailure(null);
      setProgress(p);
      if (p.done >= p.total) {
        refresh();
        clear = window.setTimeout(() => setProgress(null), 1500);
      }
    });
    return () => {
      off();
      clearTimeout(clear);
    };
  }, [backend, refresh]);

  const importFiles = useCallback(
    async (paths?: string[]) => {
      setImporting(true);
      try {
        const result = await backend.kbImport(paths);
        if (result.length) setReports(result);
        refresh();
      } catch (e) {
        notify(`导入失败：${errorMessage(e)}`, true);
      } finally {
        setImporting(false);
      }
    },
    [backend, notify, refresh],
  );

  useEffect(() => {
    if (dropped) void importFiles(dropped.paths);
  }, [dropped, importFiles]);

  const embed = () => {
    setFailure(null);
    backend.kbEmbed().catch((e) => notify(`无法开始向量化：${errorMessage(e)}`, true));
  };
  const openPath = useCallback(
    (path: string) => void backend.openPath(path).catch((e) => notify(`无法打开文件：${errorMessage(e)}`, true)),
    [backend, notify],
  );
  const remove = useCallback(
    async (doc: KbDocument) => {
      try {
        await backend.kbRemove(doc.id);
        notify(`已从知识库删除“${doc.title}”`);
        refresh();
      } catch (e) {
        notify(`删除失败：${errorMessage(e)}`, true);
      }
    },
    [backend, notify, refresh],
  );
  const updateMeta = useCallback(
    async (doc: KbDocument, meta: KbMeta) => {
      const updated = await backend.kbUpdateMeta(doc.id, meta);
      setDocuments((list) => list?.map((d) => (d.id === updated.id ? updated : d)) ?? null);
    },
    [backend],
  );

  const hasEmbedder = !!settings?.roles.embedding.model;
  const embedding = progress?.stage === "embed" && progress.done < progress.total;
  const allEmbedded = stats !== null && stats.embedded >= stats.chunks;
  const embedTitle = !hasEmbedder
    ? "请先在设置 › 模型分配中配置向量模型"
    : allEmbedded
      ? "所有片段都已向量化"
      : "为还没有向量的片段生成向量，用于语义检索";
  const busy = importing || (progress?.stage === "import" && progress.done < progress.total);

  return (
    <div className="page scroll">
      <div className="page-inner">
        <header className="page-head">
          <div className="page-title">
            <h1>知识库</h1>
            <div className="page-sub">{stats ? statsLine(stats) : "正在读取…"}</div>
          </div>
          <span className="spacer" />
          <button type="button" className="btn" disabled={busy} onClick={() => void importFiles()}>
            <FilePlus2 size={15} /> 导入资料
          </button>
          <button type="button" className="btn" disabled={!hasEmbedder || embedding || allEmbedded} title={embedTitle} onClick={() => void embed()}>
            <Zap size={15} /> 开始向量化
          </button>
        </header>

        {progress && (
          <div className="card kb-progress">
            <div className="row">
              <span className="label">
                {STAGE[progress.stage].running} {formatNumber(progress.done)} / {formatNumber(progress.total)}
              </span>
              {progress.current && <span className="muted ellipsis">{progress.current}</span>}
              {progress.message && <span className="muted">{progress.message}</span>}
            </div>
            <div className="progress">
              <span style={{ width: `${progress.total ? (progress.done / progress.total) * 100 : 0}%` }} />
            </div>
          </div>
        )}

        {failure && (
          <div className="card kb-failure" role="alert">
            <AlertTriangle size={15} />
            <span className="text">
              {STAGE[failure.stage].failed}：{failure.message}
            </span>
            <button type="button" className="icon-btn sm" title="关闭" onClick={() => setFailure(null)}>
              <X size={14} />
            </button>
          </div>
        )}

        {documents !== null && documents.length === 0 ? (
          <div className="empty-state">
            <Library size={34} strokeWidth={1.4} />
            <h2>知识库还没有资料</h2>
            <p>
              导入政策文件、通知公告、标准规范等资料。AI 修复批注时会检索其中的相关条款，在修改旁标注引用出处，点击即可打开原文核对，避免“凭空编造”。
            </p>
            <DropZone busy={busy} onPick={() => void importFiles()} />
            {reports.length > 0 && <ImportReports reports={reports} onDismiss={() => setReports([])} />}
          </div>
        ) : (
          <>
            <DropZone busy={busy} onPick={() => void importFiles()} />
            {reports.length > 0 && <ImportReports reports={reports} onDismiss={() => setReports([])} />}
            {documents && <KbDocumentTable documents={documents} onOpen={openPath} onRemove={remove} onUpdateMeta={updateMeta} />}
            <KbSearch backend={backend} onOpen={openPath} />
          </>
        )}
      </div>
    </div>
  );
});
