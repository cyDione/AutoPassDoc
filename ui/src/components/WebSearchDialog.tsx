import { useCallback, useEffect, useState } from "react";
import { Download, ExternalLink, FileText, Globe, Search, X } from "lucide-react";
import type { Backend } from "../api";
import type { Notify } from "../hooks/useDocumentSession";
import type { WebResult, WebSearchOutcome } from "../types";
import { errorMessage } from "../util";

export interface WebSearchRequest {
  /** What the "【待补充…】" asks for. */
  need: string;
  /** The text around the placeholder; the AI writes the search terms from it. */
  passage?: string;
  /** Ready-made search terms (citations), used as they are. */
  query?: string;
  nonce: number;
}

interface Props {
  backend: Backend;
  request: WebSearchRequest;
  onClose: () => void;
  notify: Notify;
}

type Download = "running" | "done" | { error: string };

function ResultRow({ r, download, onOpen, onDownload }: { r: WebResult; download: Download | undefined; onOpen: () => void; onDownload: () => void }) {
  return (
    <div className="web-result">
      <div className="title-line">
        {r.kind === "file" ? <FileText size={14} /> : <Globe size={14} />}
        <button type="button" className="link-btn title" title={`用默认浏览器打开：${r.url}`} onClick={onOpen}>
          {r.title || r.url}
        </button>
        {r.fileType && <span className="tag">{r.fileType.toUpperCase()}</span>}
      </div>
      <div className="site">{r.site}</div>
      {r.reason && <div className="snippet reason">AI：{r.reason}</div>}
      {r.snippet && <div className="snippet">{r.snippet}</div>}
      <div className="row">
        <button type="button" className="btn sm" onClick={onOpen}>
          <ExternalLink size={12} /> 在浏览器中打开
        </button>
        {r.kind === "file" &&
          (r.importable ? (
            <button type="button" className="btn sm primary" disabled={download === "running" || download === "done"} onClick={onDownload}>
              {download === "running" ? <span className="spinner sm" /> : <Download size={12} />}
              {download === "done" ? "已存入知识库" : "下载至知识库"}
            </button>
          ) : (
            <span className="muted">知识库暂不支持 .{r.fileType}，可在浏览器中打开后另存</span>
          ))}
        {typeof download === "object" && <span className="form-message error">{download.error}</span>}
      </div>
    </div>
  );
}

/** Looks up what a "【待补充…】" asks for and offers the files found to the knowledge base. */
export function WebSearchDialog({ backend, request, onClose, notify }: Props) {
  const [query, setQuery] = useState(request.query ?? "");
  const [outcome, setOutcome] = useState<WebSearchOutcome | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [searching, setSearching] = useState(false);
  const [downloads, setDownloads] = useState<ReadonlyMap<string, Download>>(() => new Map());

  // Without typed terms the AI writes them from the need and its passage.
  const search = useCallback(
    async (text: string | undefined) => {
      setSearching(true);
      setError(null);
      try {
        const found = await backend.webSearch({ query: text, need: request.need, passage: request.passage });
        setOutcome(found);
        if (!text && found.queries[0]) setQuery(found.queries[0]);
      } catch (e) {
        setError(errorMessage(e));
      } finally {
        setSearching(false);
      }
    },
    [backend, request],
  );

  useEffect(() => {
    void search(request.query);
  }, [request, search]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const setDownload = (url: string, d: Download) => setDownloads((prev) => new Map(prev).set(url, d));
  const download = async (r: WebResult) => {
    setDownload(r.url, "running");
    try {
      const report = await backend.webDownloadToKb(r.url);
      if (report.error) throw new Error(report.error);
      setDownload(r.url, "done");
      notify(report.unchanged ? `“${report.fileName}”已在知识库中` : `已把“${report.fileName}”存入知识库，重新生成时 AI 可以引用它`);
    } catch (e) {
      setDownload(r.url, { error: errorMessage(e) });
    }
  };
  const results = outcome?.results ?? [];
  const groups: [string, WebResult[]][] = [
    ["白名单网站", results.filter((r) => r.trusted)],
    ["其他网站（请核实来源）", results.filter((r) => !r.trusted)],
  ];
  const open = (url: string) => void backend.openUrl(url).catch((e) => notify(`无法打开链接：${errorMessage(e)}`, true));

  return (
    <div className="dialog-backdrop" role="presentation" onClick={onClose}>
      <div className="dialog web-dialog" role="dialog" aria-modal aria-label="查找资料" onClick={(e) => e.stopPropagation()}>
        <div className="dialog-head">
          <h2>查找资料</h2>
          <span className="muted ellipsis">需要补充：{request.need}</span>
          <span className="spacer" />
          <button type="button" className="icon-btn" title="关闭" onClick={onClose}>
            <X size={17} />
          </button>
        </div>
        <form
          className="web-query"
          onSubmit={(e) => {
            e.preventDefault();
            if (query.trim()) void search(query);
          }}
        >
          <div className="input-icon">
            <Search size={14} />
            <input className="input" value={query} onChange={(e) => setQuery(e.target.value)} />
          </div>
          <button type="submit" className="btn" disabled={searching || !query.trim()}>
            {searching && <span className="spinner sm" />}搜索
          </button>
        </form>
        <div className="dialog-content scroll web-results">
          {searching && !outcome && (
            <div className="loading">
              <span className="spinner" /> {request.query ? "正在联网搜索…" : "AI 正在拟定搜索词并联网搜索…"}
            </div>
          )}
          {error && <div className="fix-note error">{error}</div>}
          {outcome && (
            <>
              <div className="web-via muted">
                {outcome.via === "model" ? "由大语言模型联网搜索" : "由本机搜索，AI 筛选结果"}
                {outcome.queries.length > 1 && `，搜索词：${outcome.queries.join("｜")}`}
                {outcome.notes.length > 0 && `（${outcome.notes.join("；")}）`}
              </div>
              {outcome.results.length === 0 && <div className="panel-empty">没有找到结果，换个说法再试试</div>}
              {groups.map(
                ([label, rows]) =>
                  rows.length > 0 && (
                    <section key={label} className="web-group">
                      {groups[1][1].length > 0 && <div className="group-label">{label}</div>}
                      {rows.map((r) => (
                        <ResultRow key={r.url} r={r} download={downloads.get(r.url)} onOpen={() => open(r.url)} onDownload={() => void download(r)} />
                      ))}
                    </section>
                  ),
              )}
            </>
          )}
        </div>
        <div className="dialog-foot">
          <span className="muted">找到答案后，在修改里把“【待补充…】”改成实际内容再应用。</span>
          <span className="spacer" />
          <button type="button" className="btn" onClick={onClose}>
            关闭
          </button>
        </div>
      </div>
    </div>
  );
}
