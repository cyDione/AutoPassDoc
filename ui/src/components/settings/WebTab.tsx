import { useState } from "react";
import { Bot, Globe, Sparkles, WifiOff } from "lucide-react";
import type { ModelWebSearch, Settings } from "../../types";

type Web = Settings["web"];

interface Props {
  web: Web;
  onChange: (web: Web) => void;
}

const MODES = [
  ["auto", Sparkles, "自动（推荐）", "先用模型联网，不行再用白名单搜索"],
  ["model", Bot, "模型搜索", "只用大语言模型自带的联网能力"],
  ["local", Globe, "白名单搜索", "本机在白名单网站中搜索，读取网页附件"],
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

export function WebTab({ web, onChange }: Props) {
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
              {web.mode === "auto" ? "，会直接改用白名单搜索。" : "，只用模型搜索时将无法查找。"}
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
          <div className="setting-row stacked">
            <div className="setting-label">本机搜索引擎</div>
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
              <div className="setting-hint">每行一个域名，按后缀匹配：填 gov.cn 即包含所有政府网站。本机搜索只返回、只读取这些网站的内容。</div>
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
