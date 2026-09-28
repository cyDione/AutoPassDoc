// Mirrors docx-engine's view structs (crates/docx-engine/src/view.rs).

export interface Summary {
  blockCount: number;
  paragraphCount: number;
  charCount: number;
  outline: OutlineItem[];
  comments: CommentView[];
}

export interface OutlineItem {
  blockIndex: number;
  paragraphIndex: number;
  level: number;
  text: string;
}

export interface CommentView {
  id: string;
  author: string;
  initials: string | null;
  date: string | null;
  text: string;
  parentId: string | null;
  done: boolean;
  blockIndex: number | null;
  paragraphIndex: number | null;
  quote: string;
}

export type Inline =
  | { type: "text" }
  | { type: "image"; rel_id: string | null }
  | { type: "note"; id: string }
  | { type: "math" };

export interface SpanView {
  text: string;
  bold: boolean;
  italic: boolean;
  underline: boolean;
  strike: boolean;
  superscript: boolean;
  subscript: boolean;
  revision: "none" | "insert" | "delete";
  revisionAuthor?: string;
  inline: Inline;
  commentIds?: string[];
}

export interface ParagraphView {
  index: number;
  headingLevel: number | null;
  listLabel: string | null;
  align: string | null;
  spans: SpanView[];
}

export interface CellView {
  gridSpan: number;
  vMerge: boolean | null;
  paragraphs: ParagraphView[];
}

export type BlockView =
  | { kind: "paragraph"; index: number; paragraph: ParagraphView }
  | { kind: "table"; index: number; rows: CellView[][] };

export interface OpenedDoc {
  docId: number;
  path: string;
  fileName: string;
  summary: Summary;
}
