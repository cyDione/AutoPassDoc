import { memo } from "react";
import type { BlockView, CellView, ParagraphView, SpanView } from "../types";

export interface BlockProps {
  block: BlockView;
  activeCommentId: string | null;
  imageUrl: (relId: string) => string;
  onCommentClick: (ids: string[]) => void;
}

export const Block = memo(function Block({ block, ...rest }: BlockProps) {
  if (block.kind === "table") return <Table rows={block.rows} {...rest} />;
  return <Paragraph p={block.paragraph} inBody {...rest} />;
});

type ChildProps = Omit<BlockProps, "block">;

function alignClass(align: string | null): string {
  switch (align) {
    case "center":
      return " align-center";
    case "right":
    case "end":
      return " align-right";
    default:
      return "";
  }
}

function Paragraph({ p, inBody, ...rest }: ChildProps & { p: ParagraphView; inBody?: boolean }) {
  const content = (
    <>
      {p.listLabel && <span className={/[\p{L}\p{N}.]$/u.test(p.listLabel) ? "list-label spaced" : "list-label"}>{p.listLabel}</span>}
      {p.spans.map((s, i) => (
        <Span key={i} s={s} {...rest} />
      ))}
    </>
  );
  if (p.headingLevel !== null) {
    const level = Math.min(p.headingLevel, 5);
    const Tag = `h${Math.min(level + 2, 6)}` as "h2";
    return (
      <Tag className={`doc-h l${level}${alignClass(p.align)}`} data-paragraph={p.index}>
        {content}
      </Tag>
    );
  }
  const empty = p.spans.length === 0 && !p.listLabel;
  const body = inBody && !p.listLabel ? " body" : "";
  return (
    <p className={`doc-p${body}${empty ? " empty" : ""}${alignClass(p.align)}`} data-paragraph={p.index}>
      {content}
    </p>
  );
}

function Span({ s, activeCommentId, imageUrl, onCommentClick }: ChildProps & { s: SpanView }) {
  if (s.inline.type === "image") {
    return s.inline.rel_id ? <img className="doc-image" src={imageUrl(s.inline.rel_id)} alt="文档图片" loading="lazy" /> : null;
  }
  if (s.inline.type === "note") {
    return <sup className="note-ref">[{s.inline.id}]</sup>;
  }
  let cls = "";
  if (s.bold) cls += " fmt-b";
  if (s.italic) cls += " fmt-i";
  if (s.underline) cls += " fmt-u";
  if (s.strike) cls += " fmt-s";
  if (s.revision === "insert") cls += " rev-ins";
  if (s.revision === "delete") cls += " rev-del";
  const ids = s.commentIds;
  const title = s.revision !== "none" && s.revisionAuthor ? `${s.revision === "insert" ? "插入" : "删除"}：${s.revisionAuthor}` : undefined;
  let node = (
    <span className={cls.trim() || undefined} title={title}>
      {s.text}
    </span>
  );
  if (s.superscript) node = <sup>{node}</sup>;
  if (s.subscript) node = <sub>{node}</sub>;
  if (!ids?.length) return node;
  const active = activeCommentId !== null && ids.includes(activeCommentId);
  return (
    <span
      className={`commented${active ? " active" : ""}`}
      data-c={ids.join(" ")}
      onClick={(e) => {
        e.stopPropagation();
        onCommentClick(ids);
      }}
    >
      {node}
    </span>
  );
}

/** Word marks vertical merges per cell; HTML needs a rowSpan on the first cell. */
function layoutRows(rows: CellView[][]) {
  const grid = rows.map((row) => {
    let col = 0;
    return row.map((cell) => {
      const at = col;
      col += Math.max(1, cell.gridSpan);
      return { cell, col: at, rowSpan: 1, hidden: cell.vMerge === false };
    });
  });
  grid.forEach((row, r) => {
    for (const item of row) {
      if (item.cell.vMerge !== true) continue;
      for (let next = r + 1; next < grid.length; next++) {
        const below = grid[next].find((c) => c.col === item.col);
        if (!below || below.cell.vMerge !== false) break;
        item.rowSpan++;
      }
    }
  });
  return grid;
}

function Table({ rows, ...rest }: ChildProps & { rows: CellView[][] }) {
  return (
    <div className="doc-table-wrap">
      <table className="doc-table">
        <tbody>
          {layoutRows(rows).map((row, r) => (
            <tr key={r}>
              {row.map(
                ({ cell, rowSpan, hidden }, c) =>
                  !hidden && (
                    <td key={c} colSpan={cell.gridSpan > 1 ? cell.gridSpan : undefined} rowSpan={rowSpan > 1 ? rowSpan : undefined}>
                      {cell.paragraphs.map((p) => (
                        <Paragraph key={p.index} p={p} {...rest} />
                      ))}
                    </td>
                  ),
              )}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
