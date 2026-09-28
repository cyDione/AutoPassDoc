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

export function formatDate(iso: string | null): string {
  if (!iso) return "";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "";
  const pad = (v: number) => String(v).padStart(2, "0");
  return `${d.getMonth() + 1}月${d.getDate()}日 ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}
