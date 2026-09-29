import { useState } from "react";
import { CheckCircle2, Info, XCircle } from "lucide-react";
import type { Backend } from "../../api";
import { PROFILE_SOURCE, REASONING, ROLE, THINKING } from "../../labels";
import type { ModelProfileView, ModelRoleName, ModelView, ProbeResult, ProviderView, RoleModel, Settings, ThinkingLevel } from "../../types";
import { errorMessage, formatTokens } from "../../util";
import { ModelPicker } from "./ModelPicker";

const ROLES: ModelRoleName[] = ["chat", "decision", "embedding", "rerank"];

interface Props {
  backend: Backend;
  draft: Settings;
  providers: ProviderView[];
  models: ReadonlyMap<string, ModelView[]>;
  onChange: (draft: Settings) => void;
  onModel: (providerId: string, model: ModelView) => void;
  /** Saves the draft so the backend tests what is on screen. */
  beforeTest: () => Promise<void>;
}

export function RolesTab({ backend, draft, providers, models, onChange, onModel, beforeTest }: Props) {
  const setRole = (role: ModelRoleName, patch: Partial<RoleModel>) =>
    onChange({ ...draft, roles: { ...draft.roles, [role]: { ...draft.roles[role], ...patch } } });

  return (
    <div className="settings-section">
      <div className="section-head">
        <h3>模型分配</h3>
      </div>
      <p className="hint with-icon">
        <Info size={14} /> 向量、重排、Laya 等小模型可以用 Ollama、Xinference 等在本机部署，填写本机地址即可。
      </p>
      {providers.length === 0 && <p className="hint">请先在“模型服务”中添加服务商。</p>}
      {ROLES.map((role) => (
        <RoleRow
          key={role}
          role={role}
          backend={backend}
          config={draft.roles[role]}
          providers={providers}
          models={models.get(draft.roles[role].providerId) ?? []}
          decisionBackend={draft.roles.decisionBackend}
          onChange={(patch) => setRole(role, patch)}
          onDecisionBackend={(decisionBackend) => onChange({ ...draft, roles: { ...draft.roles, decisionBackend } })}
          onModel={onModel}
          beforeTest={beforeTest}
        />
      ))}
    </div>
  );
}

interface RowProps {
  role: ModelRoleName;
  backend: Backend;
  config: RoleModel;
  providers: ProviderView[];
  models: ModelView[];
  decisionBackend: Settings["roles"]["decisionBackend"];
  onChange: (patch: Partial<RoleModel>) => void;
  onDecisionBackend: (value: Settings["roles"]["decisionBackend"]) => void;
  onModel: (providerId: string, model: ModelView) => void;
  beforeTest: () => Promise<void>;
}

function capabilities(role: ModelRoleName, model: ModelView): string {
  const p = model.profile;
  const parts = [`上下文 ${formatTokens(p.contextWindow)}`];
  if (p.maxOutputTokens > 0) parts.push(`最大输出 ${formatTokens(p.maxOutputTokens)}`);
  if (role === "chat" || role === "decision") {
    const levels = p.levels.map((l) => THINKING[l]).join("/");
    parts.push(`思考：${levels || (p.reasoning === "always" ? "始终开启" : "不支持")}`);
    if (p.jsonMode) parts.push("JSON 输出");
  }
  parts.push(`来源：${PROFILE_SOURCE[p.source]}`);
  return parts.join(" · ");
}

function RoleRow({ role, backend, config, providers, models, decisionBackend, onChange, onDecisionBackend, onModel, beforeTest }: RowProps) {
  const [adjusting, setAdjusting] = useState(false);
  const [test, setTest] = useState<ProbeResult | "running" | null>(null);
  const model = models.find((m) => m.id === config.model);
  const providerMissing = config.providerId !== "" && !providers.some((p) => p.id === config.providerId);
  const judgeByChat = role === "decision" && decisionBackend === "chat";

  const runTest = async () => {
    setTest("running");
    try {
      await beforeTest();
      setTest(await backend.testRole(role));
    } catch (e) {
      setTest({ ok: false, latencyMs: 0, summary: errorMessage(e) });
    }
  };

  return (
    <div className="card role-card">
      <div className="role-head">
        <span className="role-name">{ROLE[role]}</span>
        <span className="spacer" />
        {test && test !== "running" && (
          <span className={`test-result${test.ok ? " ok" : " failed"}`} title={test.summary}>
            {test.ok ? <CheckCircle2 size={13} /> : <XCircle size={13} />}
            {test.ok ? "正常" : "失败"}
            {test.latencyMs > 0 && ` · ${test.latencyMs}ms`}
          </span>
        )}
        <button type="button" className="btn sm" disabled={test === "running" || (!config.model && !judgeByChat)} onClick={() => void runTest()}>
          {test === "running" && <span className="spinner sm" />}测试
        </button>
      </div>
      {test && test !== "running" && <div className={`test-summary${test.ok ? "" : " failed"}`}>{test.summary}</div>}

      {role === "decision" && (
        <div className="radio-group">
          <span className="field-label">评判方式</span>
          <label className="radio">
            <input type="radio" checked={decisionBackend === "jev"} onChange={() => onDecisionBackend("jev")} />
            Jev 决策接口
          </label>
          <label className="radio">
            <input type="radio" checked={decisionBackend === "chat"} onChange={() => onDecisionBackend("chat")} />
            用大语言模型评判
          </label>
        </div>
      )}

      <div className="role-pickers">
        <select
          className="select"
          value={providerMissing ? "" : config.providerId}
          onChange={(e) => onChange({ providerId: e.target.value, model: "", thinking: "" })}
        >
          <option value="">{providerMissing ? "（服务商已删除）" : "选择服务商"}</option>
          {providers.map((p) => (
            <option key={p.id} value={p.id}>
              {p.name}
            </option>
          ))}
        </select>
        <ModelPicker
          value={config.model}
          models={models}
          role={role}
          disabled={!config.providerId || providerMissing}
          placeholder={judgeByChat ? "留空则使用大语言模型" : undefined}
          onChange={(id) => onChange({ model: id, thinking: "" })}
        />
      </div>

      {config.model && (
        <div className="capability">
          <span>{model ? capabilities(role, model) : "列表中没有这个模型，按默认能力处理"}</span>
          <button type="button" className="link-btn" onClick={() => setAdjusting((v) => !v)}>
            调整
          </button>
        </div>
      )}
      {adjusting && config.model && (
        <ProfileEditor
          key={`${config.providerId}/${config.model}`}
          backend={backend}
          providerId={config.providerId}
          modelId={config.model}
          model={model}
          onSaved={(m) => {
            onModel(config.providerId, m);
            setAdjusting(false);
          }}
          onCancel={() => setAdjusting(false)}
        />
      )}

      {role === "chat" && config.model && (
        <div className="thinking-row">
          <span className="field-label">思考强度</span>
          {model && model.profile.levels.length > 0 ? (
            <div className="segmented compact">
              {(["", ...model.profile.levels] as ("" | ThinkingLevel)[]).map((level) => (
                <button
                  key={level || "default"}
                  type="button"
                  className={config.thinking === level ? "on" : ""}
                  onClick={() => onChange({ thinking: level })}
                >
                  {level ? THINKING[level] : "默认"}
                </button>
              ))}
            </div>
          ) : (
            <span className="muted">该模型不支持调节思考强度</span>
          )}
        </div>
      )}
    </div>
  );
}

interface EditorProps {
  backend: Backend;
  providerId: string;
  modelId: string;
  model: ModelView | undefined;
  onSaved: (model: ModelView) => void;
  onCancel: () => void;
}

/** Manual overrides for capabilities the provider's API does not report. */
function ProfileEditor({ backend, providerId, modelId, model, onSaved, onCancel }: EditorProps) {
  const p = model?.profile;
  const [contextWindow, setContextWindow] = useState(String(p?.contextWindow ?? 32768));
  const [maxOutput, setMaxOutput] = useState(String(p?.maxOutputTokens ?? 4096));
  const [reasoning, setReasoning] = useState<ModelProfileView["reasoning"]>(p?.reasoning ?? "none");
  const [jsonMode, setJsonMode] = useState(p?.jsonMode ?? false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const save = async (reset: boolean) => {
    const ctx = Number(contextWindow);
    const out = Number(maxOutput);
    if (!reset && (!Number.isInteger(ctx) || ctx <= 0 || !Number.isInteger(out) || out < 0)) {
      setError("请填写正整数");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      onSaved(
        await backend.setModelProfile(
          providerId,
          modelId,
          reset ? null : { contextWindow: ctx, maxOutputTokens: out, reasoning, jsonMode },
        ),
      );
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="profile-editor">
      <div className="field-grid">
        <label className="field">
          <span className="field-label">上下文长度（token）</span>
          <input className="input" type="number" min={1} value={contextWindow} onChange={(e) => setContextWindow(e.target.value)} />
        </label>
        <label className="field">
          <span className="field-label">最大输出（token）</span>
          <input className="input" type="number" min={0} value={maxOutput} onChange={(e) => setMaxOutput(e.target.value)} />
        </label>
        <label className="field">
          <span className="field-label">思考方式</span>
          <select className="select" value={reasoning} onChange={(e) => setReasoning(e.target.value as ModelProfileView["reasoning"])}>
            {Object.entries(REASONING).map(([key, label]) => (
              <option key={key} value={key}>
                {label}
              </option>
            ))}
          </select>
        </label>
        <label className="check field-check">
          <input type="checkbox" checked={jsonMode} onChange={(e) => setJsonMode(e.target.checked)} />
          支持 JSON 结构化输出
        </label>
      </div>
      <div className="form-actions">
        <button type="button" className="btn sm primary" disabled={busy} onClick={() => void save(false)}>
          保存
        </button>
        {model?.manual && (
          <button type="button" className="btn sm" disabled={busy} onClick={() => void save(true)}>
            恢复自动检测
          </button>
        )}
        <button type="button" className="btn sm ghost" disabled={busy} onClick={onCancel}>
          取消
        </button>
        {error && <span className="form-message error">{error}</span>}
      </div>
    </div>
  );
}
