import { useEffect, useState, type ReactNode } from "react";
import { CheckCircle2, ExternalLink, FileText, KeyRound, Sparkles, XCircle } from "lucide-react";
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

function ServiceCard({
  backend,
  info,
  onInfo,
  active,
  children,
}: {
  backend: Backend;
  info: ParserInfo;
  onInfo: (info: ParserInfo) => void;
  active: boolean;
  children?: ReactNode;
}) {
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
      {children}
      <div className="quota-note muted">
        {info.quotaNote}{" "}
        <Link backend={backend} url={info.consoleUrl}>
          查看用量
        </Link>
      </div>
    </div>
  );
}

const MODES = [
  ["builtin", FileText, "普通模式", "不联网，在本机解析；扫描件和图片无法提取文字"],
  ["enhanced", Sparkles, "增强模式", "PDF 和图片交给在线服务做版面分析、表格识别和 OCR，失败时退回普通模式"],
] as const;

type Service = ParserInfo["kind"];

/** Model choice and address of the chosen service, shown inside its card. */
function ServiceOptions({ kind, kb, set }: { kind: Service; kb: Kb; set: <K extends keyof Kb>(k: K, v: Kb[K]) => void }) {
  if (kind === "mineru")
    return (
      <div className="field-grid">
        <label className="field">
          <span className="field-label">模型（vlm 精度更高，pipeline 速度更快）</span>
          <select className="select" value={kb.mineruModel} onChange={(e) => set("mineruModel", e.target.value)}>
            <option value="vlm">vlm</option>
            <option value="pipeline">pipeline</option>
          </select>
        </label>
      </div>
    );
  return (
    <div className="field-grid">
      <label className="field">
        <span className="field-label">模型（PaddleOCR-VL 适合通用文档，PP-StructureV3 适合表格多的文档）</span>
        <select className="select" value={kb.paddleocrModel} onChange={(e) => set("paddleocrModel", e.target.value)}>
          <option value="PaddleOCR-VL-1.6">PaddleOCR-VL-1.6</option>
          <option value="PP-StructureV3">PP-StructureV3</option>
        </select>
      </label>
      <label className="field">
        <span className="field-label">服务地址（使用自建服务时修改）</span>
        <input className="input" value={kb.paddleocrBaseUrl} onChange={(e) => set("paddleocrBaseUrl", e.target.value)} />
      </label>
    </div>
  );
}

export function KbTab({ backend, kb, onChange }: Props) {
  const [infos, setInfos] = useState<ParserInfo[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  // The service to return to when switching from normal back to enhanced.
  const [service, setService] = useState<Service>(kb.parser === "builtin" ? "mineru" : kb.parser);

  useEffect(() => {
    let cancelled = false;
    backend
      .parserInfos()
      .then((list) => {
        if (cancelled) return;
        setInfos(list);
        if (kb.parser === "builtin") {
          const withKey = list.find((i) => i.hasKey);
          if (withKey) setService(withKey.kind);
        }
      })
      .catch((e) => !cancelled && setError(errorMessage(e)));
    return () => {
      cancelled = true;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [backend]);

  const set = <K extends keyof Kb>(k: K, v: Kb[K]) => onChange({ ...kb, [k]: v });
  const enhanced = kb.parser !== "builtin";
  const chosen = infos?.find((i) => i.kind === kb.parser);
  const pick = (kind: Service) => {
    setService(kind);
    set("parser", kind);
  };

  return (
    <div className="settings-section">
      <div className="section-head">
        <h3>知识库</h3>
        <span className={`pill sm ${enhanced && chosen?.hasKey ? "positive" : ""}`}>
          {!enhanced ? "普通模式" : chosen?.hasKey ? `增强模式（${chosen.name}）` : "增强模式（未保存 Key）"}
        </span>
      </div>
      <p className="hint">Word、TXT、Markdown 始终在本机解析，Word 表格按“列名：值”逐行分块。解析方式只影响 PDF 和图片。</p>

      <div className="setting-row stacked">
        <div className="setting-label">PDF 和图片的解析方式</div>
        <div className="choice-cards wide">
          {MODES.map(([key, Icon, label, hint]) => {
            const on = key === "enhanced" ? enhanced : !enhanced;
            return (
              <button key={key} type="button" className={`choice-card${on ? " on" : ""}`} onClick={() => set("parser", key === "enhanced" ? service : "builtin")}>
                <Icon size={18} strokeWidth={1.6} />
                <span>{label}</span>
                <small>{hint}</small>
              </button>
            );
          })}
        </div>
      </div>

      {enhanced && (
        <div className="setting-row stacked">
          <div className="setting-label">增强服务</div>
          <div className="segmented service-switch" role="tablist">
            {(infos ?? []).map((i) => (
              <button key={i.kind} type="button" role="tab" aria-selected={kb.parser === i.kind} className={kb.parser === i.kind ? "on" : ""} onClick={() => pick(i.kind)}>
                {i.name}
                {i.hasKey && <CheckCircle2 size={12} className="has-key" aria-label="已保存 Key" />}
              </button>
            ))}
          </div>
          {chosen && !chosen.hasKey && <div className="fix-note warn">还没有保存 {chosen.name} 的 Key，保存前导入仍使用普通模式。</div>}
          {chosen && (
            <ServiceCard
              key={chosen.kind}
              backend={backend}
              info={chosen}
              active
              onInfo={(next) => setInfos((list) => list?.map((i) => (i.kind === next.kind ? next : i)) ?? null)}
            >
              <ServiceOptions kind={chosen.kind} kb={kb} set={set} />
            </ServiceCard>
          )}
        </div>
      )}

      {error && <div className="form-message error">{error}</div>}
      {enhanced && !infos && !error && (
        <div className="loading">
          <span className="spinner" /> 正在加载…
        </div>
      )}
    </div>
  );
}
