import { PLACEHOLDER, type FixProposal } from "./types";

const AVATAR_COLORS = ["#c96442", "#5b7fb8", "#4f8a6b", "#a2689c", "#b8873a", "#5f8f9c", "#8c6d52", "#6b72a8"];

export function avatarColor(name: string): string {
  let h = 0;
  for (const ch of name) h = (h * 31 + ch.codePointAt(0)!) >>> 0;
  return AVATAR_COLORS[h % AVATAR_COLORS.length];
}

export function avatarText(name: string): string {
  const first = [...name.trim()][0] ?? "?";
  return /[a-z]/i.test(first) ? first.toUpperCase() : first;
}

/** 234050 → "23.4 万字" */
export function formatChars(n: number): string {
  return n >= 10000 ? `${(n / 10000).toFixed(1)} 万字` : `${n} 字`;
}

const pad = (v: number) => String(v).padStart(2, "0");

export function formatDate(iso: string | null): string {
  if (!iso) return "";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "";
  return `${d.getMonth() + 1}月${d.getDate()}日 ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

/** Unix seconds (as the Rust side stores times) → "2026-09-28" */
export function formatDay(seconds: number): string {
  const d = new Date(seconds * 1000);
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}

/** 0.864 → "86%" */
export function percent(value: number): string {
  return `${Math.round(value * 100)}%`;
}

/** 131072 → "128K", 1000000 → "1M" */
export function formatTokens(n: number): string {
  if (n >= 1_000_000) return `${+(n / 1_048_576).toFixed(1)}M`;
  if (n >= 1024) return `${Math.round(n / 1024)}K`;
  return String(n);
}

export function formatNumber(n: number): string {
  return n.toLocaleString("en-US");
}

/** Tauri rejects with plain strings; the demo with Errors. */
export function errorMessage(e: unknown): string {
  if (e instanceof Error) return e.message;
  if (typeof e === "string") return e;
  return String(e);
}

/** True when a key press belongs to a text field rather than to the app. */
export function isTyping(target: EventTarget | null): boolean {
  if (!(target instanceof HTMLElement)) return false;
  return target.isContentEditable || target.matches("input, textarea, select");
}

/** True when the proposal still holds a "【待补充…】" the user has to fill in before it can be applied. */
export function hasPlaceholder(proposal: FixProposal): boolean {
  return proposal.paragraphs.some((p) => p.new.includes(PLACEHOLDER));
}
