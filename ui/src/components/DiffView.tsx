import { memo, useState } from "react";
import type { DiffSpan } from "../types";

/** Unchanged characters kept on each side of a change. */
const CONTEXT = 30;

/** A paragraph diff: deletions struck through in red, insertions in green, long unchanged stretches folded. */
export const DiffView = memo(function DiffView({ diff }: { diff: DiffSpan[] }) {
  const [expanded, setExpanded] = useState<ReadonlySet<number>>(() => new Set());
  if (diff.every((d) => d.kind === "equal")) {
    return <div className="diff unchanged">{diff.map((d) => d.text).join("") || "（空段落）"}</div>;
  }
  return (
    <div className="diff">
      {diff.map((span, i) => {
        if (span.kind === "insert") return <ins key={i}>{span.text}</ins>;
        if (span.kind === "delete") return <del key={i}>{span.text}</del>;
        const head = i === 0 ? 0 : CONTEXT;
        const tail = i === diff.length - 1 ? 0 : CONTEXT;
        const text = span.text;
        if (expanded.has(i) || text.length <= head + tail + 8) return <span key={i}>{text}</span>;
        return (
          <span key={i}>
            {text.slice(0, head)}
            <button
              type="button"
              className="diff-gap"
              title={`展开 ${text.length - head - tail} 个未改动的字`}
              onClick={() => setExpanded((s) => new Set(s).add(i))}
            >
              …
            </button>
            {text.slice(text.length - tail)}
          </span>
        );
      })}
    </div>
  );
});
