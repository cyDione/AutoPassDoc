import { useEffect, useState } from "react";
import { CheckCircle2, ExternalLink, KeyRound, XCircle } from "lucide-react";
import type { Backend } from "../../api";
import type { ParserInfo, ParserTest, Settings } from "../../types";
import { errorMessage } from "../../util";

type Kb = Settings["kb"];

interface Props {
  backend: Backend;
  kb: Kb;
  onChange: (kb: Kb) => void;
}

const SERVICE_HINT: Record<ParserInfo["kind"], string> = {
  mineru: "上海人工智能实验室 OpenDataLab 出品，擅长复杂版面、表格和公式，单个文件不超过 200 MB、600 页。",
  paddleocr: "百度飞桨 PaddleOCR-VL，在 AI Studio 上调用，擅长扫描件和图片中的中文文字、表格识别。",
};

function Link({ backend, url, children }: { backend: Backend; url: string; children: string }) {
  return (
    <button type="button" className="link-btn" title={url} onClick={() => void backend.openUrl(url)}>
      {children} <ExternalLink size={11} />
    </button>
  );
}

function ServiceCard({ backend, info, onInfo, active }: { backend: Backend; info: ParserInfo; onInfo: (info: ParserInfo) => void; active: boolean }) {
  const [key, setKey] = useState("");
  const [busy, setBusy] = useState<"save" | "test" | "clear" | null>(null);
  const [result, setResult] = useState<ParserTest | { ok: boolean; message: string } | null>(null);

  const run = async (kind: "save" | "test" | "clear", f: () => Promise<void>) => {
    setBusy(kind);
    setResult(null);
    try {
      await f();
    } catch (e) {
      setResult({ ok: false, message: errorMessage(e) });
    } finally {
      setBusy(null);
    }
  };
  const save = () =>
    run("save", async () => {
      onInfo(await backend.setParserKey(info.kind, key.trim()));
      setKey("");
      setResult({ ok: true, message: "Key 已保存到系统钥匙串" });
    });
  const test = () => run("test", async () => setResult(await backend.testParser(info.kind, key.trim() || undefined)));
  const clear = () =>
    run("clear", async () => {
      await backend.clearParserKey(info.kind);
      onInfo({ ...info, hasKey: false });
    });

  return (
    <div className={`card parser-card${active ? " active" : ""}`}>
      <div className="parser-head">
        <strong>{info.name}</strong>
        {info.hasKey ? <span className="pill positive sm">已保存 Key</span> : <span className="pill sm">未配置</span>}
        <span className="spacer" />
        <Link backend={backend} url={info.siteUrl}>
          官网注册
        </Link>
        <Link backend={backend} url={info.keyUrl}>
          获取 Key
        </Link>
        <Link backend={backend} url={info.docsUrl}>
          API 文档
        </Link>
      </div>
      <p className="setting-hint">{SERVICE_HINT[info.kind]}</p>
      <div className="key-row">
        <div className="input-icon">
          <KeyRound size={13} />
          <input
            className="input"
            type="password"
            autoComplete="off"
            value={key}
            placeholder={info.hasKey ? "已保存，输入新 Key 可替换" : `粘贴 ${info.name} 的 API Key / Token`}
            onChange={(e) => setKey(e.target.value)}
          />
        </div>
        <button type="button" className="btn sm" disabled={!!busy || (!key.trim() && !info.hasKey)} onClick={() => void test()}>
          {busy === "test" && <span className="spinner sm" />}测试
        </button>
        <button type="button" className="btn sm primary" disabled={!!busy || !key.trim()} onClick={() => void save()}>
          {busy === "save" && <span className="spinner sm" />}保存 Key
        </button>
        {info.hasKey && (
          <button type="button" className="btn sm ghost" disabled={!!busy} onClick={() => void clear()}>
            清除
          </button>
        )}
      </div>
      {result && (
        <div className={`parser-result${result.ok ? " ok" : " error"}`}>
          {result.ok ? <CheckCircle2 size={13} /> : <XCircle size={13} />}
          <span>{result.message}</span>
          {"quota" in result && result.quota && (
            <span className="pill sm">
              剩余 {result.quota.remaining}
              {result.quota.total != null && ` / ${result.quota.total}`} {result.quota.unit}
            </span>
          )}
        </div>
      )}
      <div className="quota-note muted">
        {info.quotaNote}{" "}
        <Link backend={backend} url={info.consoleUrl}>
          查看用量
        </Link>
      </div>
    </div>
  );
}

export function KbTab({ backend, kb, onChange }: Props) {
  const [infos, setInfos] = useState<ParserInfo[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    backend
      .parserInfos()
      .then((list) => !cancelled && setInfos(list))
      .catch((e) => !cancelled && setError(errorMessage(e)));
    return () => {
      cancelled = true;
    };
  }, [backend]);

  const set = <K extends keyof Kb>(k: K, v: Kb[K]) => onChange({ ...kb, [k]: v });
  const chosen = infos?.find((i) => i.kind === kb.parser);
  const enhanced = kb.parser !== "builtin" && !!chosen?.hasKey;

  return (
    <div className="settings-section">
      <div className="section-head">
        <h3>知识库</h3>
        <span className={`pill sm ${enhanced ? "positive" : ""}`}>{enhanced ? `增强模式（${chosen?.name}）` : "普通模式"}</span>
      </div>
      <p className="hint">
        普通模式在本机解析 Word、PDF 文字层、TXT 和 Markdown，Word 表格按“列名：值”逐行分块。增强模式把 PDF 和图片交给在线服务做版面分析、表格识别和
        OCR，扫描件也能导入；在线服务失败时自动退回普通模式。Word、TXT、Markdown 始终在本机解析。
      </p>

      <div className="setting-row stacked">
        <div className="setting-label">PDF 和图片的解析方式</div>
        <div className="radio-group vertical">
          <label className="radio with-hint">
            <input type="radio" checked={kb.parser === "builtin"} onChange={() => set("parser", "builtin")} />
            <span>
              普通模式
              <small>不联网，扫描件和图片无法提取文字</small>
            </span>
          </label>
          {(infos ?? []).map((i) => (
            <label key={i.kind} className="radio with-hint">
              <input type="radio" checked={kb.parser === i.kind} onChange={() => set("parser", i.kind)} />
              <span>
                增强模式：{i.name}
                <small>{i.hasKey ? "Key 已保存" : "需要先在下方保存 Key 才会生效"}</small>
              </span>
            </label>
          ))}
        </div>
      </div>
      {kb.parser !== "builtin" && chosen && !chosen.hasKey && <div className="fix-note warn">还没有保存 {chosen.name} 的 Key，保存前导入仍使用普通模式。</div>}

      {error && <div className="form-message error">{error}</div>}
      {!infos && !error && (
        <div className="loading">
          <span className="spinner" /> 正在加载…
        </div>
      )}
      {infos?.map((info) => (
        <ServiceCard
          key={info.kind}
          backend={backend}
          info={info}
          active={kb.parser === info.kind}
          onInfo={(next) => setInfos((list) => list?.map((i) => (i.kind === next.kind ? next : i)) ?? null)}
        />
      ))}

      <div className="setting-row">
        <div className="setting-text">
          <div className="setting-label">MinerU 模型</div>
          <div className="setting-hint">vlm 精度更高；pipeline 速度更快。</div>
        </div>
        <select className="select" value={kb.mineruModel} onChange={(e) => set("mineruModel", e.target.value)}>
          <option value="vlm">vlm</option>
          <option value="pipeline">pipeline</option>
        </select>
      </div>
      <div className="setting-row">
        <div className="setting-text">
          <div className="setting-label">PaddleOCR 模型</div>
          <div className="setting-hint">PaddleOCR-VL 适合通用文档；PP-StructureV3 适合表格多的文档。</div>
        </div>
        <select className="select" value={kb.paddleocrModel} onChange={(e) => set("paddleocrModel", e.target.value)}>
          <option value="PaddleOCR-VL-1.6">PaddleOCR-VL-1.6</option>
          <option value="PP-StructureV3">PP-StructureV3</option>
        </select>
      </div>
      <div className="setting-row">
        <div className="setting-text">
          <div className="setting-label">PaddleOCR 服务地址</div>
          <div className="setting-hint">使用自建服务时修改。</div>
        </div>
        <input className="input" value={kb.paddleocrBaseUrl} onChange={(e) => set("paddleocrBaseUrl", e.target.value)} />
      </div>
    </div>
  );
}
