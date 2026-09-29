import type { Case, FixStage, ModelProfileView, ModelRoleName, ProviderKind, ThinkingLevel } from "./types";

export const FIX_STAGE: Record<FixStage, string> = {
  context: "正在读取上下文",
  retrieve: "正在检索知识库",
  generate: "正在生成修改",
  judge: "正在评判置信度",
  done: "已完成",
  error: "失败",
};

export const ROLE: Record<ModelRoleName, string> = {
  chat: "大语言模型（改写）",
  decision: "决策模型（评判置信度）",
  embedding: "向量模型（知识库检索）",
  rerank: "重排模型（检索排序）",
};

/** Short role names for tags. */
export const ROLE_SHORT: Record<ModelRoleName, string> = {
  chat: "对话",
  decision: "决策",
  embedding: "向量",
  rerank: "重排",
};

export const THINKING: Record<ThinkingLevel, string> = {
  off: "关",
  low: "低",
  medium: "中",
  high: "高",
};

export const REASONING: Record<ModelProfileView["reasoning"], string> = {
  none: "不支持思考",
  effort: "按档位（低/中/高）",
  toggle: "开关",
  budget: "思考预算",
  always: "始终思考",
};

export const PROFILE_SOURCE: Record<ModelProfileView["source"], string> = {
  fetched: "接口",
  builtin: "内置",
  manual: "手动",
  default: "默认",
};

export const PROVIDER_KINDS: { kind: ProviderKind; label: string; placeholder: string }[] = [
  { kind: "openai", label: "OpenAI 兼容", placeholder: "https://api.example.com/v1" },
  { kind: "openrouter", label: "OpenRouter", placeholder: "https://openrouter.ai/api/v1" },
  { kind: "anthropic", label: "Anthropic", placeholder: "https://api.anthropic.com" },
  { kind: "ollama", label: "Ollama（本机）", placeholder: "http://localhost:11434" },
];

export const CASE_ACTION: Record<Case["action"], { label: string; tone: "positive" | "accent" | "negative" | "neutral" }> = {
  accepted: { label: "已采纳", tone: "positive" },
  edited: { label: "修改后采纳", tone: "accent" },
  rejected: { label: "已拒绝", tone: "negative" },
  pending: { label: "待定", tone: "neutral" },
};
