import { useState } from "react";
import type { ModelWebSearch, Settings } from "../../types";

type Web = Settings["web"];

interface Props {
  web: Web;
  onChange: (web: Web) => void;
}

const MODES = [
  ["auto", "自动（推荐）", "先让大语言模型联网搜索；模型不能联网或搜索失败时，改由本机在白名单网站中搜索"],
  ["model", "只用模型搜索", "只用大语言模型自带的联网能力"],
  ["local", "只用本机白名单搜索", "由本机通过搜索引擎在白名单网站中查找，并读取网页中的附件"],
  ["off", "关闭", "不联网查找资料"],
] as const;

const MODEL_SEARCH: [ModelWebSearch | "", string][] = [
  ["", "按服务商自动识别（OpenRouter、Anthropic、阿里云百炼、智谱）"],
  ["open_router", "OpenRouter 网页插件"],
  ["anthropic", "Anthropic web_search 工具"],
  ["dash_scope", "阿里云百炼 enable_search"],
  ["zhipu", "智谱 web_search 工具"],
  ["open_ai", "OpenAI 搜索模型 web_search_options"],
];

export function WebTab({ web, onChange }: Props) {
  const [text, setText] = useState(() => web.whitelist.join("\n"));
  const set = <K extends keyof Web>(key: K, value: Web[K]) => onChange({ ...web, [key]: value });

  return (
    <div className="settings-section">
      <div className="section-head">
        <h3>联网搜索</h3>
      </div>
      <p className="hint">AI 修改里出现“【待补充…】”时，可点“查找资料”联网搜索；搜到的文件可以直接下载进知识库。文档校对核查引用文件是否废止时也会用到。</p>

      <div className="setting-row stacked">
        <div className="setting-label">搜索方式</div>
        <div className="radio-group vertical">
          {MODES.map(([key, label, hint]) => (
            <label key={key} className="radio with-hint">
              <input type="radio" checked={web.mode === key} onChange={() => set("mode", key)} />
              <span>
                {label}
                <small>{hint}</small>
              </span>
            </label>
          ))}
        </div>
      </div>

      <div className="setting-row">
        <div className="setting-text">
          <div className="setting-label">模型联网方式</div>
          <div className="setting-hint">其他 OpenAI 兼容服务（如 DeepSeek）一般不能联网，会直接改用本机搜索。</div>
        </div>
        <select
          className="select"
          value={web.modelSearch ?? ""}
          disabled={web.mode === "off" || web.mode === "local"}
          onChange={(e) => set("modelSearch", (e.target.value || null) as ModelWebSearch | null)}
        >
          {MODEL_SEARCH.map(([key, label]) => (
            <option key={key} value={key}>
              {label}
            </option>
          ))}
        </select>
      </div>

      <div className="setting-row">
        <div className="setting-text">
          <div className="setting-label">本机搜索引擎</div>
        </div>
        <div className="radio-group">
          <label className="radio">
            <input type="radio" checked={web.engine === "bing"} onChange={() => set("engine", "bing")} />
            必应
          </label>
          <label className="radio">
            <input type="radio" checked={web.engine === "baidu"} onChange={() => set("engine", "baidu")} />
            百度
          </label>
        </div>
      </div>

      <div className="setting-row stacked">
        <div className="setting-text">
          <div className="setting-label">白名单网站</div>
          <div className="setting-hint">
            每行一个域名，按后缀匹配：填 gov.cn 即包含所有政府网站。本机搜索只返回、只读取这些网站的内容。
          </div>
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
    </div>
  );
}
