import { memo, useCallback, useEffect, useRef, useState } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import type { Backend } from "../api";
import type { BlockView, OpenedDoc } from "../types";
import { Block } from "./Blocks";

/** Blocks are fetched from Rust in chunks as they scroll into view. */
const CHUNK = 100;

export interface ScrollTarget {
  blockIndex: number;
  commentId?: string;
  /** Changes on every request so repeated clicks on the same target still scroll. */
  nonce: number;
}

interface Props {
  doc: OpenedDoc;
  backend: Backend;
  activeCommentId: string | null;
  target: ScrollTarget | null;
  onCommentClick: (ids: string[]) => void;
  onTopBlockChange: (blockIndex: number) => void;
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
  backend,
  activeCommentId,
  target,
  onCommentClick,
  onTopBlockChange,
}: Props) {
  const scrollRef = useRef<HTMLDivElement>(null);
  const cache = useRef(new Map<number, BlockView>());
  const requested = useRef(new Set<number>());
  const [, setVersion] = useState(0);

  useEffect(() => {
    cache.current = new Map();
    requested.current = new Set();
    setVersion((v) => v + 1);
    scrollRef.current?.scrollTo({ top: 0 });
  }, [doc.docId]);

  const virtualizer = useVirtualizer({
    count: doc.summary.blockCount,
    getScrollElement: () => scrollRef.current,
    estimateSize: (i) => estimate(cache.current.get(i)),
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
      backend
        .blocks(doc.docId, chunk * CHUNK, (chunk + 1) * CHUNK)
        .then((blocks) => {
          for (const b of blocks) cache.current.set(b.index, b);
          setVersion((v) => v + 1);
        })
        .catch(() => requested.current.delete(chunk));
    };
    const from = Math.floor(first / CHUNK);
    const to = Math.floor(last / CHUNK);
    for (let c = from; c <= to; c++) load(c);
    load(to + 1); // prefetch the next chunk while reading
  }, [first, last, doc.docId, doc.summary.blockCount, backend]);

  const scrollOffset = virtualizer.scrollOffset ?? 0;
  const topBlock = items.find((i) => i.end > scrollOffset + 8)?.index ?? first;
  useEffect(() => onTopBlockChange(topBlock), [topBlock, onTopBlockChange]);

  useEffect(() => {
    if (!target) return;
    virtualizer.scrollToIndex(target.blockIndex, { align: target.commentId ? "center" : "start" });
    let frames = 0;
    let handle = 0;
    const settle = () => {
      const root = scrollRef.current;
      const selector = target.commentId
        ? `[data-c~="${CSS.escape(target.commentId)}"]`
        : `[data-index="${target.blockIndex}"]`;
      const el = root?.querySelector<HTMLElement>(selector);
      // Wait a few frames so measured heights settle before the final scroll.
      if (el && cache.current.has(target.blockIndex) && frames > 3) {
        el.scrollIntoView({ block: target.commentId ? "center" : "start" });
        if (target.commentId) {
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

  const imageUrl = useCallback((relId: string) => backend.imageUrl(doc.docId, relId), [backend, doc.docId]);

  return (
    <div ref={scrollRef} className="doc-scroll scroll">
      <div className="doc-canvas" style={{ height: virtualizer.getTotalSize() }}>
        {items.map((item) => {
          const block = cache.current.get(item.index);
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
