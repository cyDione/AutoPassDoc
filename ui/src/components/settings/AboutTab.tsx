import { useEffect, useState } from "react";
import { Download, ExternalLink, RefreshCw } from "lucide-react";
import type { Backend } from "../../api";
import type { AppInfo, UpdateInfo } from "../../types";
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
              {update.url && (
                <button type="button" className="btn sm primary" onClick={() => void backend.openUrl(update.url!)}>
                  <Download size={13} /> 前往下载
                </button>
              )}
            </div>
            {update.notes.trim() && <MiniMarkdown text={update.notes} />}
            {update.assets.length > 0 && (
              <div className="update-assets">
                {update.assets.map((a) => (
                  <button key={a.url} type="button" className="link-btn" onClick={() => void backend.openUrl(a.url)}>
                    {a.name}（{sizeText(a.size)}）
                  </button>
                ))}
              </div>
            )}
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
