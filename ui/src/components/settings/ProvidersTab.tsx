import { useState } from "react";
import { ChevronDown, ChevronRight, Download, Eye, EyeOff, KeyRound, Plus, Server } from "lucide-react";
import type { Backend } from "../../api";
import { PROVIDER_KINDS, ROLE_SHORT } from "../../labels";
import type { ModelView, ProviderKind, ProviderView } from "../../types";
import { errorMessage, formatTokens } from "../../util";
import { ConfirmButton } from "../ConfirmButton";

interface Props {
  backend: Backend;
  providers: ProviderView[];
  models: ReadonlyMap<string, ModelView[]>;
  onProviders: (providers: ProviderView[]) => void;
  onModels: (providerId: string, models: ModelView[]) => void;
}

const kindLabel = (kind: ProviderKind) => PROVIDER_KINDS.find((k) => k.kind === kind)?.label ?? kind;

export function ProvidersTab({ backend, providers, models, onProviders, onModels }: Props) {
  const [selected, setSelected] = useState<string | null>(providers[0]?.id ?? null);
  // A new provider keeps its form (and messages) after its first save.
  const [formKey, setFormKey] = useState(selected ?? "new");
  const current = providers.find((p) => p.id === selected) ?? null;
  const select = (id: string | null) => {
    setSelected(id);
    setFormKey(id ?? `new-${Date.now()}`);
  };

  return (
    <div className="settings-section">
      <div className="section-head">
        <h3>模型服务</h3>
        <span className="spacer" />
        <button type="button" className="btn sm" onClick={() => select(null)}>
          <Plus size={13} /> 添加服务商
        </button>
      </div>
      {providers.length === 0 && <p className="hint">还没有添加服务商。填写服务地址和 API Key 后，就能拉取可用的模型。</p>}
      {providers.length > 0 && (
        <div className="provider-list">
          {providers.map((p) => (
            <button
              key={p.id}
              type="button"
              className={`provider-item${p.id === selected ? " on" : ""}`}
              onClick={() => select(p.id)}
            >
              <Server size={15} className="icon" />
              <span className="main">
                <span className="name">
                  {p.name} <span className="kind">{kindLabel(p.kind)}</span>
                </span>
                <span className="url">{p.baseUrl}</span>
              </span>
              <span className={`badge${p.hasKey ? " done" : ""}`}>
                <KeyRound size={11} /> {p.hasKey ? "已保存密钥" : "未设置密钥"}
              </span>
            </button>
          ))}
        </div>
      )}
      <ProviderForm
        key={formKey}
        backend={backend}
        provider={current}
        models={current ? (models.get(current.id) ?? []) : []}
        onSaved={(saved) => {
          onProviders(providers.some((p) => p.id === saved.id) ? providers.map((p) => (p.id === saved.id ? saved : p)) : [...providers, saved]);
          setSelected(saved.id);
        }}
        onDeleted={(id) => {
          const rest = providers.filter((p) => p.id !== id);
          onProviders(rest);
          select(rest[0]?.id ?? null);
        }}
        onModels={onModels}
      />
    </div>
  );
}

interface FormProps {
  backend: Backend;
  /** null for a new provider. */
  provider: ProviderView | null;
  models: ModelView[];
  onSaved: (provider: ProviderView) => void;
  onDeleted: (id: string) => void;
  onModels: (providerId: string, models: ModelView[]) => void;
}

function ProviderForm({ backend, provider, models, onSaved, onDeleted, onModels }: FormProps) {
  const [name, setName] = useState(provider?.name ?? "");
  const [kind, setKind] = useState<ProviderKind>(provider?.kind ?? "openai");
  const [baseUrl, setBaseUrl] = useState(provider?.baseUrl ?? "");
  const [apiKey, setApiKey] = useState("");
  const [clearKey, setClearKey] = useState(false);
  const [showKey, setShowKey] = useState(false);
  const [advanced, setAdvanced] = useState(!!(provider?.decisionPath || provider?.rerankPath));
  const [decisionPath, setDecisionPath] = useState(provider?.decisionPath ?? "");
  const [rerankPath, setRerankPath] = useState(provider?.rerankPath ?? "");
  const [busy, setBusy] = useState<"save" | "fetch" | "delete" | null>(null);
  const [message, setMessage] = useState<{ text: string; error?: boolean } | null>(null);
  const [fetched, setFetched] = useState<number | null>(null);

  const placeholder = PROVIDER_KINDS.find((k) => k.kind === kind)!.placeholder;
  const dirty =
    !provider ||
    name !== provider.name ||
    kind !== provider.kind ||
    baseUrl !== provider.baseUrl ||
    apiKey !== "" ||
    clearKey ||
    (decisionPath.trim() || null) !== provider.decisionPath ||
    (rerankPath.trim() || null) !== provider.rerankPath;

  const save = async (): Promise<ProviderView | null> => {
    if (!name.trim()) {
      setMessage({ text: "请填写名称", error: true });
      return null;
    }
    if (!baseUrl.trim()) {
      setMessage({ text: "请填写服务地址", error: true });
      return null;
    }
    const saved = await backend.saveProvider(
      {
        id: provider?.id ?? crypto.randomUUID(),
        name: name.trim(),
        kind,
        baseUrl: baseUrl.trim(),
        decisionPath: decisionPath.trim() || null,
        rerankPath: rerankPath.trim() || null,
      },
      clearKey ? null : apiKey ? apiKey : undefined,
    );
    setApiKey("");
    setClearKey(false);
    onSaved(saved);
    return saved;
  };

  const run = async (what: "save" | "fetch" | "delete", task: () => Promise<void>) => {
    setBusy(what);
    setMessage(null);
    try {
      await task();
    } catch (e) {
      setMessage({ text: errorMessage(e), error: true });
    } finally {
      setBusy(null);
    }
  };

  const onSave = () =>
    run("save", async () => {
      if (await save()) setMessage({ text: "已保存" });
    });

  const onFetch = () =>
    run("fetch", async () => {
      const saved = dirty ? await save() : provider;
      if (!saved) return;
      const list = await backend.fetchModels(saved.id);
      onModels(saved.id, list);
      setFetched(list.length);
    });

  const onDelete = () =>
    run("delete", async () => {
      if (!provider) return;
      await backend.deleteProvider(provider.id);
      onDeleted(provider.id);
    });

  const keyStatus = clearKey ? "保存后将清除已保存的密钥" : provider?.hasKey ? "已保存密钥，留空则保持不变" : "尚未设置密钥";

  return (
    <div className="card form-card">
      <div className="form-title">{provider ? `编辑「${provider.name}」` : "添加服务商"}</div>
      <div className="field-grid">
        <label className="field">
          <span className="field-label">名称</span>
          <input className="input" value={name} placeholder="例如：公司网关、DeepSeek" onChange={(e) => setName(e.target.value)} />
        </label>
        <label className="field">
          <span className="field-label">类型</span>
          <select className="select" value={kind} onChange={(e) => setKind(e.target.value as ProviderKind)}>
            {PROVIDER_KINDS.map((k) => (
              <option key={k.kind} value={k.kind}>
                {k.label}
              </option>
            ))}
          </select>
        </label>
        <label className="field wide">
          <span className="field-label">服务地址</span>
          <input className="input mono" value={baseUrl} placeholder={placeholder} spellCheck={false} onChange={(e) => setBaseUrl(e.target.value)} />
        </label>
        <div className="field wide">
          <span className="field-label">API Key</span>
          <div className="input-group">
            <input
              className="input mono"
              type={showKey ? "text" : "password"}
              value={apiKey}
              disabled={clearKey}
              autoComplete="off"
              spellCheck={false}
              placeholder={provider?.hasKey ? "••••••••（已保存）" : kind === "ollama" ? "本机服务通常不需要" : "sk-…"}
              onChange={(e) => setApiKey(e.target.value)}
            />
            <button type="button" className="icon-btn" title={showKey ? "隐藏" : "显示"} onClick={() => setShowKey((v) => !v)}>
              {showKey ? <EyeOff size={15} /> : <Eye size={15} />}
            </button>
          </div>
          <span className="field-hint">
            {keyStatus}
            {provider?.hasKey && (
              <button type="button" className="link-btn" onClick={() => setClearKey((v) => !v)}>
                {clearKey ? "撤销清除" : "清除密钥"}
              </button>
            )}
          </span>
        </div>
      </div>
      <button type="button" className="disclosure" onClick={() => setAdvanced((v) => !v)}>
        {advanced ? <ChevronDown size={14} /> : <ChevronRight size={14} />} 高级
      </button>
      {advanced && (
        <div className="field-grid">
          <label className="field">
            <span className="field-label">决策接口路径</span>
            <input className="input mono" value={decisionPath} placeholder="systemone" onChange={(e) => setDecisionPath(e.target.value)} />
          </label>
          <label className="field">
            <span className="field-label">重排接口路径</span>
            <input className="input mono" value={rerankPath} placeholder="rerank" onChange={(e) => setRerankPath(e.target.value)} />
          </label>
        </div>
      )}
      <div className="form-actions">
        <button type="button" className="btn sm primary" disabled={busy !== null || !dirty} onClick={() => void onSave()}>
          {busy === "save" && <span className="spinner sm" />}保存
        </button>
        <button type="button" className="btn sm" disabled={busy !== null} onClick={() => void onFetch()}>
          {busy === "fetch" ? <span className="spinner sm" /> : <Download size={13} />}拉取模型列表
        </button>
        {provider && (
          <ConfirmButton className="btn sm ghost" disabled={busy !== null} confirmLabel="确认删除？" onConfirm={() => void onDelete()}>
            删除
          </ConfirmButton>
        )}
        {message && <span className={`form-message${message.error ? " error" : ""}`}>{message.text}</span>}
      </div>
      {models.length > 0 && (
        <div className="model-list">
          <div className="model-list-head">
            {fetched !== null ? `已拉取 ${fetched} 个模型` : `已缓存 ${models.length} 个模型`}
          </div>
          <div className="model-list-body scroll">
            {models.map((m) => (
              <div key={m.id} className="model-row">
                <span className="id">{m.id}</span>
                <span className="tag subtle">{ROLE_SHORT[m.roleHint]}</span>
                <span className="ctx">{formatTokens(m.profile.contextWindow)}</span>
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}
