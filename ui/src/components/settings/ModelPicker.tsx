import { useMemo, useRef, useState } from "react";
import { ChevronDown } from "lucide-react";
import { ROLE_SHORT } from "../../labels";
import type { ModelRoleName, ModelView } from "../../types";
import { formatTokens } from "../../util";
import { Floating } from "../Floating";

interface Props {
  value: string;
  models: ModelView[];
  role: ModelRoleName;
  disabled?: boolean;
  placeholder?: string;
  onChange: (modelId: string) => void;
}

/** A text field for any model id, with the provider's known models as suggestions (this role's first). */
export function ModelPicker({ value, models, role, disabled, placeholder, onChange }: Props) {
  const wrapRef = useRef<HTMLDivElement>(null);
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [highlight, setHighlight] = useState(0);

  const options = useMemo(() => {
    const q = query.trim().toLowerCase();
    return models
      .filter((m) => !q || m.id.toLowerCase().includes(q))
      .sort((a, b) => Number(b.roleHint === role) - Number(a.roleHint === role) || a.id.localeCompare(b.id));
  }, [models, query, role]);

  const choose = (id: string) => {
    onChange(id);
    setOpen(false);
  };

  return (
    <div className="model-picker" ref={wrapRef}>
      <input
        className="input mono"
        value={value}
        disabled={disabled}
        placeholder={placeholder ?? "选择或输入模型 ID"}
        spellCheck={false}
        onFocus={() => {
          setQuery("");
          setHighlight(0);
          setOpen(true);
        }}
        onBlur={() => setOpen(false)}
        onChange={(e) => {
          onChange(e.target.value);
          setQuery(e.target.value);
          setHighlight(0);
          setOpen(true);
        }}
        onKeyDown={(e) => {
          if (e.key === "ArrowDown" || e.key === "ArrowUp") {
            e.preventDefault();
            setOpen(true);
            const step = e.key === "ArrowDown" ? 1 : -1;
            setHighlight((h) => (options.length ? (h + step + options.length) % options.length : 0));
          } else if (e.key === "Enter" && open && options[highlight]) {
            e.preventDefault();
            choose(options[highlight].id);
          } else if (e.key === "Escape" && open) {
            e.stopPropagation();
            setOpen(false);
          }
        }}
      />
      <ChevronDown size={14} className="model-picker-caret" />
      {open && wrapRef.current && options.length > 0 && (
        <Floating anchor={wrapRef.current} className="dropdown">
          {options.map((m, i) => (
            <div
              key={m.id}
              className={`dropdown-item${i === highlight ? " on" : ""}${m.id === value ? " selected" : ""}`}
              onMouseDown={(e) => {
                e.preventDefault();
                choose(m.id);
              }}
              onMouseEnter={() => setHighlight(i)}
            >
              <span className="id">{m.id}</span>
              <span className={`tag${m.roleHint === role ? "" : " subtle"}`}>{ROLE_SHORT[m.roleHint]}</span>
              <span className="ctx">{formatTokens(m.profile.contextWindow)}</span>
            </div>
          ))}
        </Floating>
      )}
    </div>
  );
}
