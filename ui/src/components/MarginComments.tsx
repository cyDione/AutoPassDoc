import { memo, useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import type { FixEntries } from "../hooks/useFixes";
import type { AuthorView, CommentView, FixSelection } from "../types";
import { CommentCard, type CardActions } from "./CommentCard";
import type { MarginHost } from "./DocumentView";

interface Props {
  host: MarginHost;
  /** All comments of the document; a new list (after an edit) drops remembered anchor offsets. */
  comments: CommentView[];
  /** Threads that pass the filters, in document order. */
  threads: CommentView[];
  replies: Map<string, CommentView[]>;
  activeId: string | null;
  selection: FixSelection | null;
  authorsByName: ReadonlyMap<string, AuthorView>;
  nameOf: (author: string) => string;
  entries: FixEntries;
  directions: ReadonlyMap<string, string>;
  actions: CardActions;
}

/** Space between stacked cards. */
const GAP = 6;
/** Cards are drawn this far above and below the viewport. */
const OVERSCAN = 700;
/** Height used for a card that has not been drawn yet. */
const COLLAPSED_GUESS = 36;
const ACTIVE_GUESS = 220;
/** Cards sit a little above the first line of their anchor so their text lines up with it. */
const NUDGE = 5;

interface Placed {
  id: string;
  top: number;
}

interface Layout {
  /** Cards to draw: the ones near the viewport, plus the active one. */
  shown: Placed[];
  /** Bottom of the last card, so the margin can scroll past the end of the text. */
  bottom: number;
  /** Line from the active comment's highlight to its card, in margin coordinates. */
  connector: string | null;
}

const EMPTY: Layout = { shown: [], bottom: 0, connector: null };

function sameLayout(a: Layout, b: Layout): boolean {
  if (a.bottom !== b.bottom || a.connector !== b.connector || a.shown.length !== b.shown.length) return false;
  return a.shown.every((p, i) => p.id === b.shown[i].id && p.top === b.shown[i].top);
}

/**
 * Classic Word comment layout (C-4): each thread's card sits in the page margin
 * at the height of the text it marks. Anchors of blocks that are not rendered
 * (the document is virtualized) are placed from the block's estimated start plus
 * the offset seen the last time the block was on screen. Overlapping cards are
 * pushed down in document order; the selected card keeps its own height and the
 * cards above it move up to make room.
 */
export const MarginComments = memo(function MarginComments({
  host,
  comments,
  threads,
  replies,
  activeId,
  selection,
  authorsByName,
  nameOf,
  entries,
  directions,
  actions,
}: Props) {
  const [layout, setLayout] = useState<Layout>(EMPTY);
  const heights = useRef(new Map<string, number>());
  /** Last seen position of each anchor: the block it starts in and its offset from the block's top. */
  const anchors = useRef(new Map<string, { block: number; offset: number }>());
  const collapsedHeight = useRef(COLLAPSED_GUESS);
  const input = useRef({ threads, activeId });
  const frame = useRef(0);
  const [settling, setSettling] = useState(false);

  // Offsets seen before an edit may no longer hold.
  useEffect(() => {
    anchors.current.clear();
  }, [comments]);

  const compute = useCallback(() => {
    const { threads, activeId } = input.current;
    const origin = host.cards.getBoundingClientRect();
    const view = host.scroller.getBoundingClientRect();

    // Anchors in the blocks on screen: the first highlight of each comment.
    const seen = new Map<string, { y: number; block: number; blockTop: number; el: HTMLElement }>();
    const blockTops = new Map<HTMLElement, number>();
    for (const el of host.scroller.querySelectorAll<HTMLElement>(".block [data-c]")) {
      const blockEl = el.closest<HTMLElement>(".block");
      if (!blockEl) continue;
      const block = Number(blockEl.dataset.index);
      let blockTop = blockTops.get(blockEl);
      if (blockTop === undefined) {
        blockTop = blockEl.getBoundingClientRect().top - origin.top;
        blockTops.set(blockEl, blockTop);
      }
      const rect = el.getClientRects()[0];
      if (!rect) continue;
      const y = rect.top - origin.top;
      for (const id of (el.dataset.c ?? "").split(" ")) {
        const prev = seen.get(id);
        if (!prev || block < prev.block || (block === prev.block && y < prev.y)) seen.set(id, { y, block, blockTop, el });
      }
    }

    // Where each card would like to be.
    const wanted: { id: string; y: number; order: number }[] = [];
    let last = 0;
    threads.forEach((t, order) => {
      const s = seen.get(t.id);
      let y: number | null = null;
      if (s && (t.blockIndex === null || s.block <= t.blockIndex)) {
        y = s.y;
        anchors.current.set(t.id, { block: s.block, offset: s.y - s.blockTop });
      } else {
        const known = anchors.current.get(t.id);
        const block = known?.block ?? t.blockIndex;
        const start = block === null ? null : host.blockStart(block);
        if (start !== null) y = start + (known?.offset ?? 0);
      }
      last = y ?? last;
      wanted.push({ id: t.id, y: last - NUDGE, order });
    });
    wanted.sort((a, b) => a.y - b.y || a.order - b.order);

    const heightOf = (id: string) => heights.current.get(id) ?? (id === activeId ? ACTIVE_GUESS : collapsedHeight.current);
    const minTop = host.bar.offsetHeight + GAP;
    const tops = new Array<number>(wanted.length);
    const pushDown = (from: number, to: number, floor: number) => {
      for (let i = from; i < to; i++) {
        tops[i] = Math.max(wanted[i].y, floor);
        floor = tops[i] + heightOf(wanted[i].id) + GAP;
      }
      return floor;
    };
    const a = activeId ? wanted.findIndex((w) => w.id === activeId) : -1;
    if (a < 0) {
      pushDown(0, wanted.length, minTop);
    } else {
      // Cards above the selected one stack as usual, then move up so they end above it.
      pushDown(0, a, minTop);
      tops[a] = Math.max(wanted[a].y, minTop);
      for (let i = a - 1; i >= 0; i--) tops[i] = Math.min(tops[i], tops[i + 1] - GAP - heightOf(wanted[i].id));
      if (a > 0 && tops[0] < minTop) {
        // No room above: stack from the top again, which pushes the selected card down.
        pushDown(0, wanted.length, minTop);
      } else {
        pushDown(a + 1, wanted.length, tops[a] + heightOf(wanted[a].id) + GAP);
      }
    }

    const viewTop = view.top - origin.top - OVERSCAN;
    const viewBottom = view.bottom - origin.top + OVERSCAN;
    const shown: Placed[] = [];
    let bottom = 0;
    wanted.forEach((w, i) => {
      const h = heightOf(w.id);
      bottom = Math.max(bottom, tops[i] + h);
      if (w.id === activeId || (tops[i] + h >= viewTop && tops[i] <= viewBottom)) shown.push({ id: w.id, top: Math.round(tops[i]) });
    });

    // A thin line from the selected highlight across the page's right edge to its card.
    let connector: string | null = null;
    const s = activeId ? seen.get(activeId) : undefined;
    if (s && a >= 0) {
      const r = s.el.getClientRects()[0];
      const x0 = Math.round(r.right - origin.left);
      const y0 = Math.round(r.top + r.height / 2 - origin.top);
      const y1 = Math.round(tops[a] + 18);
      connector = `${x0},${y0} -6,${y0} 0,${y1}`;
    }

    const next: Layout = { shown, bottom: Math.round(bottom), connector };
    setLayout((prev) => (sameLayout(prev, next) ? prev : next));
  }, [host]);

  const scheduleRef = useRef<() => void>(() => {});
  const schedule = useCallback(() => {
    if (frame.current) return;
    frame.current = requestAnimationFrame(() => {
      frame.current = 0;
      compute();
    });
  }, [compute]);

  useLayoutEffect(() => {
    scheduleRef.current = schedule;
  }, [schedule]);

  // New filters or another selection: lay out before paint.
  useLayoutEffect(() => {
    input.current = { threads, activeId };
    compute();
  }, [threads, activeId, compute]);

  // Cards glide to their new places after a selection, but not while scrolling.
  useEffect(() => {
    if (!activeId) return;
    setSettling(true);
    const t = setTimeout(() => setSettling(false), 320);
    return () => clearTimeout(t);
  }, [activeId]);

  useEffect(() => {
    const unsubscribe = host.subscribe(schedule);
    const resize = new ResizeObserver(schedule);
    resize.observe(host.scroller);
    resize.observe(host.bar);
    host.scroller.addEventListener("scroll", schedule, { passive: true });
    return () => {
      unsubscribe();
      resize.disconnect();
      host.scroller.removeEventListener("scroll", schedule);
      cancelAnimationFrame(frame.current);
      frame.current = 0;
    };
  }, [host, schedule]);

  // Card heights change when a card expands, shows a fix or wraps differently.
  const [cardObserver] = useState(
    () =>
      new ResizeObserver((records) => {
        let changed = false;
        for (const r of records) {
          const el = r.target as HTMLElement;
          const id = el.dataset.id;
          if (!id) continue;
          const h = r.borderBoxSize?.[0]?.blockSize ?? el.offsetHeight;
          if (h === 0) continue;
          if (!el.classList.contains("active")) collapsedHeight.current = h;
          if (Math.abs((heights.current.get(id) ?? -1) - h) > 0.5) {
            heights.current.set(id, h);
            changed = true;
          }
        }
        if (changed) scheduleRef.current();
      }),
  );
  const observe = useCallback(
    (el: HTMLDivElement | null) => {
      if (!el) return;
      cardObserver.observe(el);
      return () => cardObserver.unobserve(el);
    },
    [cardObserver],
  );

  const byId = new Map(threads.map((t) => [t.id, t]));

  return createPortal(
    <div className={`margin-layer${settling ? " settling" : ""}`} style={{ height: layout.bottom }}>
      {layout.connector && (
        <svg className="margin-connector" width="1" height="1" aria-hidden>
          <polyline points={layout.connector} />
        </svg>
      )}
      {layout.shown.map(({ id, top }) => {
        const t = byId.get(id);
        if (!t) return null;
        const active = id === activeId;
        return (
          <div
            key={id}
            ref={observe}
            data-id={id}
            className={`margin-item${active ? " active" : ""}`}
            style={{ transform: `translateY(${top}px)` }}
          >
            <CommentCard
              comment={t}
              replies={replies.get(id)}
              active={active}
              selection={active ? selection : null}
              author={authorsByName.get(t.author)}
              nameOf={nameOf}
              fix={entries.get(id)}
              direction={directions.get(id) ?? ""}
              actions={actions}
            />
          </div>
        );
      })}
    </div>,
    host.cards,
  );
});
