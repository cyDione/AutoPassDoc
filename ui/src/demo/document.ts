import type { BlockView, CommentView, DiffSpan, DocState, EditOutcome, ParagraphView, SpanView, Summary } from "../types";

/** The sample report as exported by `pnpm demo-data`; never mutated. */
export interface DemoSource {
  summary: Summary;
  blocks: BlockView[];
}

interface Snapshot {
  blocks: BlockView[];
  comments: CommentView[];
}

interface Edit {
  id: number;
  label: string;
  before: Snapshot;
  after: Snapshot;
}

let nextEditId = 1;

/** One open document with an undo history, all in memory. */
export class DemoDocument {
  readonly docId: number;
  readonly path: string;
  readonly fileName: string;
  private summaryBase: Summary;
  private blockList: BlockView[];
  comments: CommentView[];
  private undoStack: Edit[] = [];
  private redoStack: Edit[] = [];
  private savedEdit = 0;
  savedPath: string | null = null;
  /** paragraph index → block index, for paragraphs in body and table cells. */
  private paragraphBlock = new Map<number, number>();

  constructor(docId: number, path: string, source: DemoSource) {
    this.docId = docId;
    this.path = path;
    this.fileName = path.split(/[\\/]/).pop() ?? path;
    this.summaryBase = source.summary;
    this.blockList = source.blocks.slice();
    this.comments = source.summary.comments;
    for (const block of this.blockList) {
      for (const p of paragraphsOf(block)) this.paragraphBlock.set(p.index, block.index);
    }
  }

  get summary(): Summary {
    return { ...this.summaryBase, comments: this.comments };
  }

  get state(): DocState {
    const top = this.undoStack[this.undoStack.length - 1];
    return {
      dirty: (top?.id ?? 0) !== this.savedEdit,
      undoLabel: top?.label ?? null,
      redoLabel: this.redoStack[this.redoStack.length - 1]?.label ?? null,
      savedPath: this.savedPath,
    };
  }

  get outcome(): EditOutcome {
    return { state: this.state, summary: this.summary };
  }

  blocks(start: number, end: number): BlockView[] {
    return this.blockList.slice(start, end);
  }

  markSaved(path: string) {
    this.savedPath = path;
    this.savedEdit = this.undoStack[this.undoStack.length - 1]?.id ?? 0;
  }

  paragraph(index: number): ParagraphView | null {
    const at = this.paragraphBlock.get(index);
    if (at === undefined) return null;
    for (const p of paragraphsOf(this.blockList[at])) if (p.index === index) return p;
    return null;
  }

  /** Paragraph indices start..end (exclusive) that exist, in order. */
  paragraphsIn(start: number, end: number): ParagraphView[] {
    const out: ParagraphView[] = [];
    for (let i = start; i < end; i++) {
      const p = this.paragraph(i);
      if (p) out.push(p);
    }
    return out;
  }

  commit(label: string, change: { paragraphs?: Map<number, SpanView[]>; comments?: CommentView[] }): EditOutcome {
    const touched = new Map<number, BlockView>();
    for (const [index, spans] of change.paragraphs ?? []) {
      const at = this.paragraphBlock.get(index);
      if (at === undefined) continue;
      touched.set(at, withParagraph(touched.get(at) ?? this.blockList[at], index, spans));
    }
    const before: Snapshot = { blocks: [...touched.keys()].map((i) => this.blockList[i]), comments: this.comments };
    const after: Snapshot = { blocks: [...touched.values()], comments: change.comments ?? this.comments };
    this.restore(after);
    this.undoStack.push({ id: nextEditId++, label, before, after });
    this.redoStack = [];
    return this.outcome;
  }

  undo(): EditOutcome {
    const edit = this.undoStack.pop();
    if (!edit) throw new Error("没有可撤销的操作");
    this.restore(edit.before);
    this.redoStack.push(edit);
    return this.outcome;
  }

  redo(): EditOutcome {
    const edit = this.redoStack.pop();
    if (!edit) throw new Error("没有可重做的操作");
    this.restore(edit.after);
    this.undoStack.push(edit);
    return this.outcome;
  }

  private restore(snapshot: Snapshot) {
    for (const block of snapshot.blocks) this.blockList[block.index] = block;
    this.comments = snapshot.comments;
  }
}

export function paragraphsOf(block: BlockView): ParagraphView[] {
  if (block.kind === "paragraph") return [block.paragraph];
  return block.rows.flatMap((row) => row.flatMap((cell) => cell.paragraphs));
}

function withParagraph(block: BlockView, index: number, spans: SpanView[]): BlockView {
  if (block.kind === "paragraph") return { ...block, paragraph: { ...block.paragraph, spans } };
  return {
    ...block,
    rows: block.rows.map((row) =>
      row.map((cell) =>
        cell.paragraphs.some((p) => p.index === index)
          ? { ...cell, paragraphs: cell.paragraphs.map((p) => (p.index === index ? { ...p, spans } : p)) }
          : cell,
      ),
    ),
  };
}

const visible = (s: SpanView) => s.inline.type === "text" && s.revision !== "delete";

/** The paragraph's current text: tracked deletions and inline objects left out. */
export function paragraphText(p: ParagraphView): string {
  return p.spans.filter(visible).map((s) => s.text).join("");
}

/**
 * Rewrites a paragraph's spans along a diff of its text. Formatting and comment
 * ranges carry over; tracked mode marks the change as Word revisions.
 */
export function rewriteSpans(spans: SpanView[], diff: DiffSpan[], tracked: boolean, author: string): SpanView[] {
  const out: SpanView[] = [];
  let at = 0; // next span
  let offset = 0; // characters of spans[at] already consumed
  let last: SpanView | null = null;

  const passHidden = () => {
    while (at < spans.length && !visible(spans[at])) out.push(spans[at++]);
  };
  const take = (length: number, map: (s: SpanView, text: string) => SpanView | null) => {
    while (length > 0 && at < spans.length) {
      passHidden();
      const s = spans[at];
      if (!s) break;
      const n = Math.min(length, s.text.length - offset);
      const piece = map(s, s.text.slice(offset, offset + n));
      if (piece) out.push(piece);
      last = s;
      length -= n;
      offset += n;
      if (offset >= s.text.length) {
        at++;
        offset = 0;
      }
    }
  };
  const revision = (s: SpanView, text: string, kind: "insert" | "delete"): SpanView => ({
    ...s,
    text,
    revision: kind,
    revisionAuthor: author,
    inline: { type: "text" },
  });

  for (const op of diff) {
    if (op.kind === "equal") take(op.text.length, (s, text) => ({ ...s, text }));
    else if (op.kind === "delete") take(op.text.length, (s, text) => (tracked ? revision(s, text, "delete") : null));
    else {
      if (!last) passHidden();
      const base: SpanView = last ?? spans.find(visible) ?? plainSpan();
      if (tracked) out.push(revision(base, op.text, "insert"));
      else {
        const { revisionAuthor: _drop, ...rest } = base;
        out.push({ ...rest, text: op.text, revision: "none", inline: { type: "text" } });
      }
    }
  }
  while (at < spans.length) out.push(spans[at++]);
  return merge(out);
}

function plainSpan(): SpanView {
  return {
    text: "",
    bold: false,
    italic: false,
    underline: false,
    strike: false,
    superscript: false,
    subscript: false,
    revision: "none",
    inline: { type: "text" },
  };
}

function sameFormat(a: SpanView, b: SpanView): boolean {
  return (
    a.inline.type === "text" &&
    b.inline.type === "text" &&
    a.bold === b.bold &&
    a.italic === b.italic &&
    a.underline === b.underline &&
    a.strike === b.strike &&
    a.superscript === b.superscript &&
    a.subscript === b.subscript &&
    a.revision === b.revision &&
    a.revisionAuthor === b.revisionAuthor &&
    (a.commentIds ?? []).join() === (b.commentIds ?? []).join()
  );
}

function merge(spans: SpanView[]): SpanView[] {
  const out: SpanView[] = [];
  for (const s of spans) {
    if (s.inline.type === "text" && !s.text) continue;
    const prev = out[out.length - 1];
    if (prev && sameFormat(prev, s)) out[out.length - 1] = { ...prev, text: prev.text + s.text };
    else out.push(s);
  }
  return out;
}
