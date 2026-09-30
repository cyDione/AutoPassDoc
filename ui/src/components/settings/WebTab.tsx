import { useEffect, useState } from "react";
import { Bot, ExternalLink, Globe, KeyRound, Sparkles, WifiOff } from "lucide-react";
import type { Backend } from "../../api";
import type { ModelWebSearch, SearchService, SearchServiceInfo, Settings } from "../../types";
import { errorMessage } from "../../util";

type Web = Settings["web"];

interface Props {
  backend: Backend;
  web: Web;
  onChange: (web: Web) => void;
}

/** The chosen search API with its key, like ZCode's and other agent apps' search services. */
function SearchServiceRow({ backend, service, onPick }: { backend: Backend; service: SearchService; onPick: (s: SearchService) => void }) {
  const [infos, setInfos] = useState<SearchServiceInfo[]>([]);
  const [key, setKey] = useState("");
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<{ ok: boolean; text: string } | null>(null);
  useEffect(() => {
    backend.searchServices().then(setInfos, (e) => setMessage({ ok: false, text: errorMessage(e) }));
  }, [backend]);
  const info = infos.find((i) => i.kind === service);
  const run = async (f: () => Promise<SearchServiceInfo[]>, ok: string) => {
    setBusy(true);
    setMessage(null);
    try {
      setInfos(await f());
      setKey("");
      setMessage({ ok: true, text: ok });
    } catch (e) {
      setMessage({ ok: false, text: errorMessage(e) });
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="setting-row stacked">
      <div className="setting-text">
        <div className="setting-label">搜索 API（推荐）</div>
        <div className="setting-hint">
          专为 AI 设计的搜索接口，结果稳定，不会被当成机器人拦截。配置后优先使用它，必应、百度只在它失败时兜底。政府网站站内搜索（如上海市政府公文库）总会先查。
        </div>
      </div>
      <div className="segmented compact">
        {(["none", ...infos.map((i) => i.kind)] as SearchService[]).map((k) => (
          <button key={k} type="button" className={service === k ? "on" : ""} onClick={() => onPick(k)}>
            {k === "none" ? "不使用" : (infos.find((i) => i.kind === k)?.name ?? k)}
            {k !== "none" && infos.find((i) => i.kind === k)?.hasKey && " ✓"}
          </button>
        ))}
      </div>
      {info && (
        <>
          <div className="setting-hint">
            {info.note}{" "}
            <button type="button" className="link-btn" title={info.keyUrl} onClick={() => void backend.openUrl(info.keyUrl)}>
              获取 Key <ExternalLink size={11} />
            </button>
          </div>
          <div className="key-row">
            <div className="input-icon">
              <KeyRound size={13} />
              <input
                className="input"
                type="password"
                autoComplete="off"
                value={key}
                placeholder={info.hasKey ? "已保存，输入新 Key 可替换" : `粘贴 ${info.name} 的 API Key`}
                onChange={(e) => setKey(e.target.value)}
              />
            </div>
            <button type="button" className="btn sm primary" disabled={busy || !key.trim()} onClick={() => void run(() => backend.setSearchKey(info.kind, key.trim()), "Key 已保存")}>
              {busy && <span className="spinner sm" />}保存 Key
            </button>
            {info.hasKey && (
              <button type="button" className="btn sm ghost" disabled={busy} onClick={() => void run(() => backend.clearSearchKey(info.kind), "Key 已清除")}>
                清除
              </button>
            )}
          </div>
        </>
      )}
      {message && <div className={`form-message${message.ok ? "" : " error"}`}>{message.text}</div>}
    </div>
  );
}

const MODES = [
  ["auto", Sparkles, "自动（推荐）", "先用模型联网，不行再用本机搜索"],
  ["model", Bot, "模型搜索", "只用大语言模型自带的联网能力"],
  ["local", Globe, "本机搜索", "搜索 API、政府网站站内搜索和搜索引擎，白名单网站优先"],
  ["off", WifiOff, "关闭", "不联网查找资料"],
] as const;

const MODEL_SEARCH: [ModelWebSearch | "", string][] = [
  ["", "按服务商自动识别（OpenRouter、Anthropic、阿里云百炼、智谱）"],
  ["open_router", "OpenRouter 网页插件"],
  ["anthropic", "Anthropic web_search 工具"],
  ["dash_scope", "阿里云百炼 enable_search"],
  ["zhipu", "智谱 web_search 工具"],
  ["open_ai", "OpenAI 搜索模型 web_search_options"],
];

const ENGINES = [
  ["bing", "必应"],
  ["baidu", "百度"],
] as const;

export function WebTab({ backend, web, onChange }: Props) {
  const [text, setText] = useState(() => web.whitelist.join("\n"));
  const set = <K extends keyof Web>(key: K, value: Web[K]) => onChange({ ...web, [key]: value });
  const useModel = web.mode === "auto" || web.mode === "model";
  const useLocal = web.mode === "auto" || web.mode === "local";

  return (
    <div className="settings-section">
      <div className="section-head">
        <h3>联网搜索</h3>
      </div>
      <p className="hint">AI 修改里出现“【待补充…】”时，可点“查找资料”联网搜索；搜到的文件可以直接下载进知识库。文档校对核查引用文件是否废止时也会用到。</p>

      <div className="setting-row stacked">
        <div className="setting-label">搜索方式</div>
        <div className="choice-cards">
          {MODES.map(([key, Icon, label, hint]) => (
            <button key={key} type="button" className={`choice-card${web.mode === key ? " on" : ""}`} onClick={() => set("mode", key)}>
              <Icon size={18} strokeWidth={1.6} />
              <span>{label}</span>
              <small>{hint}</small>
            </button>
          ))}
        </div>
      </div>

      {useModel && (
        <div className="setting-row stacked">
          <div className="setting-text">
            <div className="setting-label">模型联网方式</div>
            <div className="setting-hint">
              按“模型分配”里大语言模型的服务商选择开启联网的参数。其他 OpenAI 兼容服务（如 DeepSeek）一般不能联网
              {web.mode === "auto" ? "，会直接改用本机搜索。" : "，只用模型搜索时将无法查找。"}
            </div>
          </div>
          <select className="select" value={web.modelSearch ?? ""} onChange={(e) => set("modelSearch", (e.target.value || null) as ModelWebSearch | null)}>
            {MODEL_SEARCH.map(([key, label]) => (
              <option key={key} value={key}>
                {label}
              </option>
            ))}
          </select>
        </div>
      )}

      {useLocal && (
        <>
          <SearchServiceRow backend={backend} service={web.service ?? "none"} onPick={(s) => set("service", s)} />
          <div className="setting-row stacked">
            <div className="setting-text">
              <div className="setting-label">备用搜索引擎</div>
              <div className="setting-hint">直接读取搜索结果网页，可能被要求人机验证；另一个引擎会自动补上。</div>
            </div>
            <div className="segmented compact">
              {ENGINES.map(([key, label]) => (
                <button key={key} type="button" className={web.engine === key ? "on" : ""} onClick={() => set("engine", key)}>
                  {label}
                </button>
              ))}
            </div>
          </div>
          <div className="setting-row stacked">
            <div className="setting-text">
              <div className="setting-label">白名单网站</div>
              <div className="setting-hint">每行一个域名，按后缀匹配：填 gov.cn 即包含所有政府网站。这些网站的结果排在最前，AI 只读取它们的网页原文，附件也只能从这些网站下载。</div>
            </div>
            <textarea
              className="textarea mono"
              rows={10}
              value={text}
              onChange={(e) => {
                setText(e.target.value);
                set(
                  "whitelist",
                  e.target.value
                    .split(/[\n,，\s]+/)
                    .map((s) => s.trim())
                    .filter(Boolean),
                );
              }}
            />
          </div>
        </>
      )}
      {web.mode === "off" && <div className="panel-empty">已关闭联网。“查找资料”和校对中的引用时效核查将不可用。</div>}
    </div>
  );
}
