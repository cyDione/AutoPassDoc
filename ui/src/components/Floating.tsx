import { useEffect, useLayoutEffect, useRef, useState, type CSSProperties, type ReactNode } from "react";
import { createPortal } from "react-dom";

interface Props {
  anchor: HTMLElement;
  /** Called on a click outside the layer and its anchor, or on Escape. */
  onClose?: () => void;
  /** Fixed width; defaults to the anchor's width. */
  width?: number;
  align?: "start" | "end";
  /**
   * Height to plan for when choosing a side. For layers whose content arrives
   * after they open (a list still loading), so they do not open below the
   * anchor and then get squeezed once the content is in.
   */
  expectedHeight?: number;
  className?: string;
  children: ReactNode;
}

const MARGIN = 8;
const GAP = 6;

/**
 * A layer drawn above everything (popovers, dropdowns, hover cards), placed
 * below its anchor or above it when there is more room there. Rendered in a
 * portal so scroll containers do not clip it.
 */
export function Floating({ anchor, onClose, width, align = "start", expectedHeight = 0, className, children }: Props) {
  const ref = useRef<HTMLDivElement>(null);
  const [style, setStyle] = useState<CSSProperties>({ visibility: "hidden", top: 0, left: 0 });

  useLayoutEffect(() => {
    let below: boolean | null = null;
    const place = () => {
      const el = ref.current;
      if (!el) return;
      const a = anchor.getBoundingClientRect();
      const w = width ?? a.width;
      const roomBelow = window.innerHeight - a.bottom - GAP - MARGIN;
      const roomAbove = a.top - GAP - MARGIN;
      // Decide once, from the natural height, so the layer does not jump while its content changes.
      below ??= Math.max(el.scrollHeight, expectedHeight) <= roomBelow || roomBelow >= roomAbove;
      const left = Math.min(Math.max(MARGIN, align === "end" ? a.right - w : a.left), window.innerWidth - w - MARGIN);
      setStyle(
        below
          ? { top: a.bottom + GAP, left, width: w, maxHeight: Math.max(120, roomBelow) }
          : { bottom: window.innerHeight - a.top + GAP, left, width: w, maxHeight: Math.max(120, roomAbove) },
      );
    };
    place();
    const onScroll = (e: Event) => {
      if (!ref.current?.contains(e.target as Node)) place();
    };
    window.addEventListener("resize", place);
    window.addEventListener("scroll", onScroll, true);
    return () => {
      window.removeEventListener("resize", place);
      window.removeEventListener("scroll", onScroll, true);
    };
  }, [anchor, width, align, expectedHeight]);

  useEffect(() => {
    if (!onClose) return;
    const onPointer = (e: MouseEvent) => {
      const target = e.target as Node;
      if (!ref.current?.contains(target) && !anchor.contains(target)) onClose();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        onClose();
      }
    };
    document.addEventListener("mousedown", onPointer);
    document.addEventListener("keydown", onKey, true);
    return () => {
      document.removeEventListener("mousedown", onPointer);
      document.removeEventListener("keydown", onKey, true);
    };
  }, [anchor, onClose]);

  return createPortal(
    <div ref={ref} className={`floating${className ? ` ${className}` : ""}`} style={style}>
      {children}
    </div>,
    document.body,
  );
}
