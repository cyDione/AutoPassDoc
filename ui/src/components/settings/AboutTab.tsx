import { useEffect, useState } from "react";
import { Download, ExternalLink, RefreshCw, X } from "lucide-react";
import type { Backend } from "../../api";
import type { AppInfo, UpdateInfo, UpdateProgress } from "../../types";
import { errorMessage } from "../../util";
import { MiniMarkdown } from "../MiniMarkdown";
import changelog from "../../../../CHANGELOG.md?raw";

const RELEASES = "https://github.com/cyDione/AutoPassDoc/releases";

function sizeText(bytes: number): string {
  return bytes >= 1_000_000 ? `${(bytes / 1_048_576).toFixed(1)} MB` : `${Math.max(1, Math.round(bytes / 1024))} KB`;
}

export function AboutTab({ backend }: { backend: Backend }) {
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [checking, setChecking] = useState(false);
  const [update, setUpdate] = useState<UpdateInfo | null>(null);
  const [error, setError] = useState<string | null>(null);
  // In-app update: downloading → installing (the app quits once the installer starts).
  const [phase, setPhase] = useState<"idle" | "downloading" | "installing">("idle");
  const [progress, setProgress] = useState<UpdateProgress | null>(null);
  const [installError, setInstallError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    backend
      .appInfo()
      .then((i) => !cancelled && setInfo(i))
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [backend]);

  useEffect(() => backend.onUpdateProgress(setProgress), [backend]);

  const install = async () => {
    const installer = update?.installer;
    if (!update || !installer) return;
    const ok = await backend.confirm(
      `将下载 ${update.latest} 版安装包（${sizeText(installer.size)}），下载完成后 AutoPassDoc 会自动关闭并安装，装好后重新打开。请先保存正在编辑的文档。`,
      "更新到新版本",
      "下载并安装",
    );
    if (!ok) return;
    setInstallError(null);
    setProgress(null);
    setPhase("downloading");
    try {
      const path = await backend.downloadUpdate(installer);
      setPhase("installing");
      await backend.installUpdate(path);
    } catch (e) {
      setInstallError(errorMessage(e));
      setPhase("idle");
    }
  };

  const percent = progress && progress.total > 0 ? Math.min(100, Math.round((progress.downloaded / progress.total) * 100)) : 0;

  const check = async () => {
    setChecking(true);
    setError(null);
    try {
      setUpdate(await backend.checkUpdate());
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setChecking(false);
    }
  };

  return (
    <div className="settings-section">
      <div className="section-head">
        <h3>关于</h3>
      </div>
      <div className="card about-card">
        <div className="about-title">
          <strong>AutoPassDoc</strong>
          <span className="pill sm">版本 {info?.version ?? "…"}</span>
        </div>
        <p className="setting-hint">按审稿批注修改 Word 报告的桌面工具。</p>
        {info && (
          <div className="about-path muted">
            数据目录：<span className="mono">{info.dataDir}</span>
          </div>
        )}
        <div className="form-actions">
          <button type="button" className="btn sm" disabled={checking} onClick={() => void check()}>
            {checking ? <span className="spinner sm" /> : <RefreshCw size={13} />}检查更新
          </button>
          <button type="button" className="btn sm ghost" onClick={() => void backend.openUrl(RELEASES)}>
            <ExternalLink size={13} /> 所有版本
          </button>
          {error && <span className="form-message error">检查失败：{error}</span>}
          {update && !update.hasUpdate && (
            <span className="form-message">{update.latest ? `已是最新版本（${update.current}）` : "GitHub 上还没有发布版本"}</span>
          )}
        </div>
        {update?.hasUpdate && (
          <div className="update-box">
            <div className="update-head">
              <strong>发现新版本 {update.latest}</strong>
              {update.publishedAt && <span className="muted">发布于 {update.publishedAt.slice(0, 10)}</span>}
              <span className="spacer" />
              {update.installer ? (
                phase === "idle" && (
                  <button type="button" className="btn sm primary" onClick={() => void install()}>
                    <Download size={13} /> 下载并安装
                  </button>
                )
              ) : (
                update.url && (
                  <button type="button" className="btn sm" onClick={() => void backend.openUrl(update.url!)}>
                    <ExternalLink size={13} /> 前往下载
                  </button>
                )
              )}
            </div>
            {phase === "downloading" && (
              <div className="update-progress">
                <div className="progress">
                  <span style={{ width: `${percent}%` }} />
                </div>
                <div className="update-progress-row muted">
                  <span>
                    正在下载 {progress ? `${sizeText(progress.downloaded)} / ${sizeText(progress.total)}（${percent}%）` : "…"}
                  </span>
                  <span className="spacer" />
                  <button type="button" className="btn sm ghost" onClick={() => void backend.cancelUpdateDownload()}>
                    <X size={13} /> 取消
                  </button>
                </div>
              </div>
            )}
            {phase === "installing" && (
              <div className="update-progress-row">
                <span className="spinner sm" /> 正在启动安装程序，AutoPassDoc 即将关闭…
              </div>
            )}
            {installError && <div className="form-message error">更新失败：{installError}</div>}
            {!update.installer && <p className="setting-hint">这个版本没有适用于当前系统的安装包，请前往发布页手动下载。</p>}
            {update.notes.trim() && <MiniMarkdown text={update.notes} />}
          </div>
        )}
      </div>
      <div className="section-head">
        <h3>更新日志</h3>
      </div>
      <div className="card changelog">
        <MiniMarkdown text={changelog.replace(/^[\s\S]*?(?=^## )/m, "")} />
      </div>
    </div>
  );
}
