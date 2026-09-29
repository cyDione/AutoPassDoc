import { useMemo, useState, type ReactNode } from "react";
import { ExternalLink, FileText, Search } from "lucide-react";
import type { Backend } from "../../api";
import type { KbHit } from "../../types";
import { errorMessage } from "../../util";

/** Wraps every occurrence of the query's terms in <mark>. */
function highlight(text: string, query: string): ReactNode {
  const terms = query.split(/\s+/).filter(Boolean);
  if (terms.length === 0) return text;
  const pattern = new RegExp(`(${terms.map((t) => t.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")).join("|")})`, "gi");
  return text.split(pattern).map((part, i) => (i % 2 === 1 ? <mark key={i}>{part}</mark> : part));
}

interface Props {
  backend: Backend;
  onOpen: (path: string) => void;
}

/** "检索测试": runs the same hybrid search a fix would, to check what the knowledge base returns. */
export function KbSearch({ backend, onOpen }: Props) {
  const [query, setQuery] = useState("");
  const [result, setResult] = useState<{ query: string; hits: KbHit[] } | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const search = async () => {
    const text = query.trim();
    if (!text) return;
    setBusy(true);
    setError(null);
    try {
      setResult({ query: text, hits: await backend.kbSearch(text) });
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  const hits = useMemo(
    () =>
      result?.hits.map((h) => (
        <div key={h.chunkId} className="hit">
          <div className="hit-head">
            <FileText size={14} className="icon" />
            <span className="hit-title" title={h.fileName}>
              {h.title}
            </span>
            <span className="spacer" />
            <button type="button" className="btn sm" onClick={() => onOpen(h.storedPath)}>
              <ExternalLink size={12} /> 打开
            </button>
          </div>
          {h.headingPath.length > 0 && <div className="hit-path">{h.headingPath.join(" › ")}</div>}
          <div className="hit-text">{highlight(h.text, result.query)}</div>
          <div className="hit-badges">
            <span className="tag subtle">得分 {h.score.toFixed(3)}</span>
            {h.rerankScore !== null && <span className="tag">重排 {h.rerankScore.toFixed(2)}</span>}
            {h.keywordRank !== null && <span className="tag subtle">关键词 #{h.keywordRank}</span>}
            {h.vectorRank !== null && <span className="tag subtle">向量 #{h.vectorRank}</span>}
          </div>
        </div>
      )),
    [result, onOpen],
  );

  return (
    <section className="card kb-search">
      <div className="section-head">
        <h3>检索测试</h3>
        <span className="muted">输入批注或原文中的说法，看看知识库会找到哪些条款</span>
      </div>
      <form
        className="search-row"
        onSubmit={(e) => {
          e.preventDefault();
          void search();
        }}
      >
        <div className="input-icon">
          <Search size={15} />
          <input className="input" value={query} placeholder="例如：数据共享率、投资估算、等级保护" onChange={(e) => setQuery(e.target.value)} />
        </div>
        <button type="submit" className="btn" disabled={busy || !query.trim()}>
          {busy && <span className="spinner sm" />}检索
        </button>
      </form>
      {error && <div className="form-message error">{error}</div>}
      {result && (
        <div className="hits">
          <div className="muted small">{result.hits.length ? `找到 ${result.hits.length} 个片段` : "没有找到相关片段"}</div>
          {hits}
        </div>
      )}
    </section>
  );
}
