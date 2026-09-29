import { Fragment, type ReactNode } from "react";

/** Inline `**bold**`, `code` and [text](url) (shown as text). */
function inline(text: string): ReactNode[] {
  const out: ReactNode[] = [];
  const re = /\*\*(.+?)\*\*|`([^`]+)`|\[([^\]]+)\]\([^)]+\)/g;
  let last = 0;
  let m: RegExpExecArray | null;
  while ((m = re.exec(text))) {
    if (m.index > last) out.push(text.slice(last, m.index));
    if (m[1]) out.push(<strong key={m.index}>{m[1]}</strong>);
    else if (m[2]) out.push(<code key={m.index}>{m[2]}</code>);
    else out.push(m[3]);
    last = m.index + m[0].length;
  }
  if (last < text.length) out.push(text.slice(last));
  return out;
}

/** Renders the small Markdown subset used by the changelog and release notes. */
export function MiniMarkdown({ text, skipTitle = false }: { text: string; skipTitle?: boolean }) {
  const blocks: ReactNode[] = [];
  let list: string[] = [];
  const flush = () => {
    if (list.length === 0) return;
    const items = list;
    blocks.push(
      <ul key={`ul${blocks.length}`}>
        {items.map((item, i) => (
          <li key={i}>{inline(item)}</li>
        ))}
      </ul>,
    );
    list = [];
  };
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trimEnd();
    const heading = /^(#{1,4})\s+(.*)$/.exec(line);
    const item = /^\s*[-*]\s+(.*)$/.exec(line);
    if (item) {
      list.push(item[1]);
      continue;
    }
    flush();
    if (!line.trim()) continue;
    if (heading) {
      const level = heading[1].length;
      if (level === 1 && skipTitle) continue;
      const Tag = (level <= 2 ? "h4" : "h5") as "h4" | "h5";
      blocks.push(<Tag key={blocks.length}>{inline(heading[2])}</Tag>);
    } else blocks.push(<p key={blocks.length}>{inline(line)}</p>);
  }
  flush();
  return <div className="mini-md">{blocks.map((b, i) => <Fragment key={i}>{b}</Fragment>)}</div>;
}
