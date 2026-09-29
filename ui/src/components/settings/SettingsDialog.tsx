import { useCallback, useEffect, useState } from "react";
import { Cpu, Database, Server, Sparkles, X } from "lucide-react";
import type { Backend } from "../../api";
import type { ModelView, ProviderView, Settings } from "../../types";
import { errorMessage } from "../../util";
import { DataTab } from "./DataTab";
import { FixTab } from "./FixTab";
import { ProvidersTab } from "./ProvidersTab";
import { RolesTab } from "./RolesTab";

export type SettingsTab = "providers" | "roles" | "fix" | "data";

const TABS = [
  ["providers", Server, "模型服务"],
  ["roles", Cpu, "模型分配"],
  ["fix", Sparkles, "AI 修复"],
  ["data", Database, "数据"],
] as const;

interface Props {
  backend: Backend;
  initialTab: SettingsTab;
  onClose: () => void;
  onSaved: (settings: Settings) => void;
}

export function SettingsDialog({ backend, initialTab, onClose, onSaved }: Props) {
  const [tab, setTab] = useState<SettingsTab>(initialTab);
  const [saved, setSaved] = useState<Settings | null>(null);
  const [draft, setDraft] = useState<Settings | null>(null);
  const [providers, setProviders] = useState<ProviderView[] | null>(null);
  const [models, setModels] = useState<ReadonlyMap<string, ModelView[]>>(() => new Map());
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      const [settings, list] = await Promise.all([backend.settings(), backend.providers()]);
      const cached = await Promise.all(list.map((p) => backend.providerModels(p.id).catch(() => [])));
      if (cancelled) return;
      setSaved(settings);
      setDraft(settings);
      setProviders(list);
      setModels(new Map(list.map((p, i) => [p.id, cached[i]])));
    })().catch((e) => !cancelled && setError(errorMessage(e)));
    return () => {
      cancelled = true;
    };
  }, [backend]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose]);

  const setModelList = useCallback((providerId: string, list: ModelView[]) => {
    setModels((prev) => new Map(prev).set(providerId, list));
  }, []);

  const setModel = useCallback((providerId: string, model: ModelView) => {
    setModels((prev) => {
      const list = prev.get(providerId) ?? [];
      const next = list.some((m) => m.id === model.id) ? list.map((m) => (m.id === model.id ? model : m)) : [...list, model];
      return new Map(prev).set(providerId, next);
    });
  }, []);

  const persist = async (settings: Settings) => {
    const result = await backend.saveSettings(settings);
    setSaved(result);
    onSaved(result);
    return result;
  };

  // Tests run against saved settings, so unsaved role changes are saved first.
  const beforeTest = async () => {
    if (draft && JSON.stringify(draft.roles) !== JSON.stringify(saved?.roles)) {
      await persist({ ...(saved ?? draft), roles: draft.roles });
    }
  };

  const save = async () => {
    if (!draft) return;
    setSaving(true);
    setError(null);
    try {
      await persist(draft);
      onClose();
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setSaving(false);
    }
  };

  const ready = draft && providers;

  return (
    <div className="dialog-backdrop" role="presentation">
      <div className="dialog settings-dialog" role="dialog" aria-modal aria-label="设置">
        <div className="dialog-head">
          <h2>设置</h2>
          <span className="spacer" />
          <button type="button" className="icon-btn" title="关闭" onClick={onClose}>
            <X size={17} />
          </button>
        </div>
        <div className="dialog-body">
          <nav className="dialog-tabs">
            {TABS.map(([key, Icon, label]) => (
              <button key={key} type="button" className={`side-action${tab === key ? " on" : ""}`} onClick={() => setTab(key)}>
                <Icon size={15} /> {label}
              </button>
            ))}
          </nav>
          <div key={tab} className="dialog-content scroll">
            {!ready && !error && (
              <div className="loading">
                <span className="spinner" /> 正在加载…
              </div>
            )}
            {ready && tab === "providers" && (
              <ProvidersTab backend={backend} providers={providers} models={models} onProviders={setProviders} onModels={setModelList} />
            )}
            {ready && tab === "roles" && (
              <RolesTab
                backend={backend}
                draft={draft}
                providers={providers}
                models={models}
                onChange={setDraft}
                onModel={setModel}
                beforeTest={beforeTest}
              />
            )}
            {ready && tab === "fix" && <FixTab fix={draft.fix} onChange={(fix) => setDraft({ ...draft, fix })} />}
            {tab === "data" && <DataTab backend={backend} />}
          </div>
        </div>
        <div className="dialog-foot">
          {error && <span className="form-message error">{error}</span>}
          <span className="spacer" />
          <button type="button" className="btn" onClick={onClose}>
            取消
          </button>
          <button type="button" className="btn primary" disabled={!draft || saving} onClick={() => void save()}>
            {saving && <span className="spinner sm" />}保存
          </button>
        </div>
      </div>
    </div>
  );
}
