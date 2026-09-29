import { memo, useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import type { Backend } from "../api";
import type { BlockView, FixSelection, OpenedDoc } from "../types";
import { Block } from "./Blocks";

/** Blocks are fetched from Rust in chunks as they scroll into view. */
const CHUNK = 100;

export interface ScrollTarget {
  blockIndex: number;
  commentId?: string;
  /** Flashes this paragraph once it is in view. */
  paragraphIndex?: number;
  /** Changes on every request so repeated clicks on the same target still scroll. */
  nonce: number;
  /** Leaves the scroll position alone when the target is already in view (it still flashes). */
  soft?: boolean;
}

/**
 * The page margin of the classic Word comment layout (C-4). It lives inside the
 * document's scroll container, so whatever is placed in it scrolls with the text.
 * Coordinates are relative to the top of the scrolled content.
 */
export interface MarginHost {
  /** Cards go here; its top is the top of the scrolled content. */
  cards: HTMLElement;
  /** A bar that sticks to the top of the margin while the document scrolls. */
  bar: HTMLElement;
  scroller: HTMLElement;
  /** Where a block starts: measured once it has been rendered, estimated before. */
  blockStart: (blockIndex: number) => number | null;
  /** Called after every render of the document view (scroll, loaded blocks, measured heights). */
  subscribe: (listener: () => void) => () => void;
}

interface Props {
  doc: OpenedDoc;
  /** Changes after every edit; the view then refetches its blocks. */
  version: number;
  backend: Backend;
  activeCommentId: string | null;
  target: ScrollTarget | null;
  onCommentClick: (ids: string[]) => void;
  onTopBlockChange: (blockIndex: number) => void;
  /** Text selected in the document, or null once the selection collapses. */
  onSelectionChange: (selection: FixSelection | null) => void;
  /** Draws a comment margin to the right of the page (classic Word layout). */
  margin?: boolean;
  onMarginHost?: (host: MarginHost | null) => void;
}

function paragraphOf(root: HTMLElement, node: Node | null): number | null {
  const el = node instanceof Element ? node : node?.parentElement;
  const p = el?.closest<HTMLElement>("[data-paragraph]");
  return p && root.contains(p) ? Number(p.dataset.paragraph) : null;
}

/** Rough height before a block has been measured, so the scrollbar is close to right. */
function estimate(block: BlockView | undefined): number {
  if (!block) return 96;
  if (block.kind === "table") return 60 + block.rows.length * 44;
  const p = block.paragraph;
  if (p.headingLevel !== null) return p.headingLevel === 0 ? 78 : 52;
  const chars = p.spans.reduce((n, s) => n + s.text.length, 0);
  if (p.spans.some((s) => s.inline.type === "image")) return 240;
  return Math.max(1, Math.ceil(chars / 40)) * 33 + 14;
}

export const DocumentView = memo(function DocumentView({
  doc,
  version,
  backend,
  activeCommentId,
  target,
  onCommentClick,
  onTopBlockChange,
  onSelectionChange,
  margin = false,
  onMarginHost,
}: Props) {
  const scrollRef = useRef<HTMLDivElement>(null);
  /** The comment margin (classic Word layout): its sticky bar and the layer its cards go in. */
  const marginBarRef = useRef<HTMLDivElement>(null);
  const marginCardsRef = useRef<HTMLDivElement>(null);
  const cache = useRef(new Map<number, BlockView>());
  /** Blocks from before the last edit, drawn until their fresh copy arrives so heights and scroll position hold. */
  const stale = useRef(new Map<number, BlockView>());
  const requested = useRef(new Set<number>());
  /** Responses to requests from an older document or version are dropped. */
  const generation = useRef(0);
  const seenVersion = useRef(version);
  const [, setTick] = useState(0);

  useEffect(() => {
    cache.current = new Map();
    stale.current = new Map();
    requested.current = new Set();
    generation.current++;
    setTick((v) => v + 1);
    scrollRef.current?.scrollTo({ top: 0 });
  }, [doc.docId]);

  useEffect(() => {
    if (seenVersion.current === version) return;
    seenVersion.current = version;
    for (const [i, b] of cache.current) stale.current.set(i, b);
    cache.current = new Map();
    requested.current = new Set();
    generation.current++;
    setTick((v) => v + 1);
  }, [version]);

  const blockAt = (i: number) => cache.current.get(i) ?? stale.current.get(i);

  const virtualizer = useVirtualizer({
    count: doc.summary.blockCount,
    getScrollElement: () => scrollRef.current,
    estimateSize: (i) => estimate(blockAt(i)),
    overscan: 6,
    paddingStart: 40,
    paddingEnd: 200,
  });

  const items = virtualizer.getVirtualItems();
  const first = items[0]?.index ?? 0;
  const last = items[items.length - 1]?.index ?? 0;

  useEffect(() => {
    const load = (chunk: number) => {
      if (chunk < 0 || chunk * CHUNK >= doc.summary.blockCount || requested.current.has(chunk)) return;
      requested.current.add(chunk);
      const gen = generation.current;
      backend
        .blocks(doc.docId, chunk * CHUNK, (chunk + 1) * CHUNK)
        .then((blocks) => {
          if (gen !== generation.current) return;
          for (const b of blocks) {
            cache.current.set(b.index, b);
            stale.current.delete(b.index);
          }
          setTick((v) => v + 1);
        })
        .catch(() => gen === generation.current && requested.current.delete(chunk));
    };
    const from = Math.floor(first / CHUNK);
    const to = Math.floor(last / CHUNK);
    for (let c = from; c <= to; c++) load(c);
    load(to + 1); // prefetch the next chunk while reading
  }, [first, last, doc.docId, doc.summary.blockCount, backend, version]);

  const scrollOffset = virtualizer.scrollOffset ?? 0;
  const topBlock = items.find((i) => i.end > scrollOffset + 8)?.index ?? first;
  useEffect(() => onTopBlockChange(topBlock), [topBlock, onTopBlockChange]);

  useEffect(() => {
    if (!target) return;
    const centered = target.commentId !== undefined || target.paragraphIndex !== undefined;
    const selector =
      target.commentId !== undefined
        ? `[data-c~="${CSS.escape(target.commentId)}"]`
        : target.paragraphIndex !== undefined
          ? `[data-paragraph="${target.paragraphIndex}"]`
          : `[data-index="${target.blockIndex}"]`;
    let inView = false;
    if (target.soft && scrollRef.current) {
      const view = scrollRef.current.getBoundingClientRect();
      const r = scrollRef.current.querySelector<HTMLElement>(selector)?.getBoundingClientRect();
      // The margin's sticky bar covers the top of the view (and the card placed beside the text there).
      const inset = margin ? (marginBarRef.current?.offsetHeight ?? 0) : 0;
      inView = !!r && r.top >= view.top + inset + 24 && r.bottom <= view.bottom - 24;
    }
    if (!inView) virtualizer.scrollToIndex(target.blockIndex, { align: centered ? "center" : "start" });
    let frames = 0;
    let handle = 0;
    const settle = () => {
      const root = scrollRef.current;
      const el = root?.querySelector<HTMLElement>(selector);
      // Wait a few frames so measured heights settle before the final scroll.
      if (el && blockAt(target.blockIndex) && (inView || frames > 3)) {
        if (!inView) el.scrollIntoView({ block: centered ? "center" : "start" });
        if (centered) {
          for (const n of root!.querySelectorAll<HTMLElement>(selector)) {
            n.classList.remove("flash");
            void n.offsetWidth;
            n.classList.add("flash");
          }
        }
        return;
      }
      if (frames++ < 120) handle = requestAnimationFrame(settle);
    };
    handle = requestAnimationFrame(settle);
    return () => cancelAnimationFrame(handle);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [target?.nonce]);

  // Selections made in the document; one that moves elsewhere (a click in the
  // comments panel) keeps the last document selection.
  useEffect(() => {
    const root = scrollRef.current;
    if (!root) return;
    const onChange = () => {
      const sel = document.getSelection();
      if (!sel || sel.rangeCount === 0 || !root.contains(sel.anchorNode)) return;
      // Clicks and typing in the comment margin must not drop the document selection.
      const at = sel.anchorNode instanceof Element ? sel.anchorNode : sel.anchorNode?.parentElement;
      if (at?.closest(".doc-margin")) return;
      const a = paragraphOf(root, sel.anchorNode);
      const b = paragraphOf(root, sel.focusNode);
      const text = sel.isCollapsed ? "" : sel.toString().trim();
      if (a === null || b === null || !text) return onSelectionChange(null);
      onSelectionChange({ startParagraph: Math.min(a, b), endParagraph: Math.max(a, b), text });
    };
    document.addEventListener("selectionchange", onChange);
    return () => document.removeEventListener("selectionchange", onChange);
  }, [onSelectionChange]);

  // An edit or another document makes the old selection meaningless.
  useEffect(() => onSelectionChange(null), [doc.docId, version, onSelectionChange]);

  // The comment margin: its host is handed out while the margin is shown, and
  // listeners hear about every render so cards follow the blocks.
  const listeners = useRef(new Set<() => void>());
  const virtualizerRef = useRef(virtualizer);
  useLayoutEffect(() => {
    virtualizerRef.current = virtualizer;
  });
  useLayoutEffect(() => {
    if (!margin || !onMarginHost || !marginBarRef.current || !marginCardsRef.current || !scrollRef.current) return;
    const subscribers = listeners.current;
    onMarginHost({
      cards: marginCardsRef.current,
      bar: marginBarRef.current,
      scroller: scrollRef.current,
      blockStart: (i) => virtualizerRef.current.measurementsCache[i]?.start ?? null,
      subscribe: (listener) => {
        subscribers.add(listener);
        return () => subscribers.delete(listener);
      },
    });
    return () => onMarginHost(null);
  }, [margin, onMarginHost]);
  useEffect(() => {
    for (const listener of listeners.current) listener();
  });

  const imageUrl = useCallback((relId: string) => backend.imageUrl(doc.docId, relId), [backend, doc.docId]);

  return (
    <div ref={scrollRef} className={`doc-scroll scroll${margin ? " with-margin" : ""}`}>
      <div className="doc-canvas" style={{ height: virtualizer.getTotalSize() }}>
        {margin && (
          <div className="doc-margin">
            <div className="doc-margin-bar" ref={marginBarRef} />
            <div className="doc-margin-cards" ref={marginCardsRef} />
          </div>
        )}
        {items.map((item) => {
          const block = blockAt(item.index);
          return (
            <div
              key={item.key}
              data-index={item.index}
              ref={virtualizer.measureElement}
              className="block"
              style={{ transform: `translateY(${item.start}px)` }}
            >
              <div className="block-inner">
                {block ? (
                  <Block block={block} activeCommentId={activeCommentId} imageUrl={imageUrl} onCommentClick={onCommentClick} />
                ) : (
                  <div className="block-skeleton" />
                )}
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
});
