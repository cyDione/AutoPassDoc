import { useCallback, useEffect, useState, type PointerEvent } from "react";

const KEY = "commentsPanelWidth";
const MIN = 300;
/** The document keeps at least this much room. */
const DOC_MIN = 420;

function clamp(width: number): number {
  const max = Math.max(MIN, window.innerWidth - DOC_MIN - 264);
  return Math.round(Math.min(Math.max(width, MIN), max));
}

function readWidth(): number | null {
  try {
    const v = Number(localStorage.getItem(KEY));
    return v > 0 ? v : null;
  } catch {
    return null;
  }
}

/**
 * Width of the comments panel, dragged from its left edge and remembered.
 * Null means the stylesheet's default; a double click on the edge restores it.
 */
export function usePanelWidth() {
  const [width, setWidth] = useState<number | null>(readWidth);
  const [resizing, setResizing] = useState(false);

  useEffect(() => {
    try {
      if (width === null) localStorage.removeItem(KEY);
      else localStorage.setItem(KEY, String(width));
    } catch {
      /* storage unavailable */
    }
  }, [width]);

  // Keep a remembered width inside a smaller window.
  useEffect(() => {
    const fit = () => setWidth((w) => (w === null ? w : clamp(w)));
    fit();
    window.addEventListener("resize", fit);
    return () => window.removeEventListener("resize", fit);
  }, []);

  const onPointerDown = useCallback((e: PointerEvent<HTMLElement>) => {
    if (e.button !== 0) return;
    e.preventDefault();
    const handle = e.currentTarget;
    handle.setPointerCapture(e.pointerId);
    const right = handle.parentElement!.getBoundingClientRect().right;
    setResizing(true);
    const move = (ev: globalThis.PointerEvent) => setWidth(clamp(right - ev.clientX));
    const up = () => {
      setResizing(false);
      handle.removeEventListener("pointermove", move);
      handle.removeEventListener("pointerup", up);
      handle.removeEventListener("pointercancel", up);
    };
    handle.addEventListener("pointermove", move);
    handle.addEventListener("pointerup", up);
    handle.addEventListener("pointercancel", up);
  }, []);

  const reset = useCallback(() => setWidth(null), []);

  return { width, resizing, onPointerDown, reset };
}
