import { useCallback, useEffect, useRef, useState } from "react";
import type { Backend } from "../api";
import type { FixProposal, FixSelection, FixStage } from "../types";
import { errorMessage } from "../util";
import type { Notify, RunEdit } from "./useDocumentSession";

/** The AI fix state of one comment. */
export type FixEntry =
  /** `stage` is null while queued. */
  | { kind: "running"; stage: FixStage | null }
  | { kind: "ready"; proposal: FixProposal; busy?: boolean; error?: string }
  | { kind: "applied" }
  | { kind: "failed"; error: string };

export type FixEntries = ReadonlyMap<string, FixEntry>;

/** How long a card shows "已应用" before it falls back to the comment's own state. */
const APPLIED_MS = 2500;

function withEntry(map: FixEntries, id: string, entry: FixEntry | null): FixEntries {
  const next = new Map(map);
  if (entry) next.set(id, entry);
  else next.delete(id);
  return next;
}

export interface FixActions {
  /**
   * Runs a fix. `selection` rewrites those paragraphs instead of the ones under
   * the highlight; left out, a regenerate reuses the comment's last selection.
   */
  fix: (commentId: string, selection?: FixSelection | null) => void;
  apply: (commentId: string, proposal: FixProposal, edited: string[] | null, force: boolean) => Promise<boolean>;
  reject: (commentId: string, proposal: FixProposal) => void;
}

export function useFixes(backend: Backend, docId: number, edit: RunEdit, notify: Notify) {
  const [entries, setEntries] = useState<FixEntries>(() => new Map());
  /** Comment ids of the latest batch, while its bar is shown. */
  const [batch, setBatch] = useState<string[] | null>(null);
  const [applyingAll, setApplyingAll] = useState<{ done: number; total: number } | null>(null);
  const entriesRef = useRef(entries);
  const timers = useRef(new Set<number>());
  const selections = useRef(new Map<string, FixSelection>());

  useEffect(() => {
    entriesRef.current = entries;
  }, [entries]);

  useEffect(() => {
    const pending = timers.current;
    return () => pending.forEach(clearTimeout);
  }, []);

  const ready = useCallback((commentId: string, proposal: FixProposal) => {
    setEntries((prev) => {
      const current = prev.get(commentId);
      if (current?.kind === "ready" && current.proposal.id === proposal.id) return prev;
      return withEntry(prev, commentId, { kind: "ready", proposal });
    });
  }, []);

  useEffect(
    () =>
      backend.onFixProgress((p) => {
        if (p.docId !== docId) return;
        if (p.stage === "done") {
          if (p.proposal) ready(p.commentId, p.proposal);
        } else if (p.stage === "error") {
          setEntries((prev) => withEntry(prev, p.commentId, { kind: "failed", error: p.error ?? p.message ?? "修复失败" }));
        } else {
          const stage = p.stage;
          setEntries((prev) => withEntry(prev, p.commentId, { kind: "running", stage }));
        }
      }),
    [backend, docId, ready],
  );

  const fix = useCallback(
    async (commentId: string, selection?: FixSelection | null) => {
      if (selection) selections.current.set(commentId, selection);
      else if (selection === null) selections.current.delete(commentId);
      setEntries((prev) => withEntry(prev, commentId, { kind: "running", stage: null }));
      try {
        ready(commentId, await backend.fixComment(docId, commentId, selections.current.get(commentId) ?? null));
      } catch (e) {
        setEntries((prev) => withEntry(prev, commentId, { kind: "failed", error: errorMessage(e) }));
      }
    },
    [backend, docId, ready],
  );

  const apply = useCallback(
    async (commentId: string, proposal: FixProposal, edited: string[] | null, force: boolean) => {
      setEntries((prev) => withEntry(prev, commentId, { kind: "ready", proposal, busy: true }));
      try {
        await edit(() => backend.applyFix(docId, proposal.id, edited, force));
        setEntries((prev) => withEntry(prev, commentId, { kind: "applied" }));
        const timer = window.setTimeout(() => {
          timers.current.delete(timer);
          setEntries((prev) => (prev.get(commentId)?.kind === "applied" ? withEntry(prev, commentId, null) : prev));
        }, APPLIED_MS);
        timers.current.add(timer);
        return true;
      } catch (e) {
        setEntries((prev) => withEntry(prev, commentId, { kind: "ready", proposal, error: errorMessage(e) }));
        return false;
      }
    },
    [backend, docId, edit],
  );

  const reject = useCallback(
    (commentId: string, proposal: FixProposal) => {
      setEntries((prev) => withEntry(prev, commentId, null));
      backend.rejectFix(proposal.id).catch(() => {});
    },
    [backend],
  );

  const runBatch = useCallback(
    async (ids: string[]) => {
      if (ids.length === 0) return;
      setEntries((prev) => {
        const next = new Map(prev);
        for (const id of ids) next.set(id, { kind: "running", stage: null });
        return next;
      });
      setBatch(ids);
      try {
        await backend.fixBatch(docId, ids);
      } catch (e) {
        const error = errorMessage(e);
        notify(`批量修复失败：${error}`, true);
        setEntries((prev) => {
          const next = new Map(prev);
          for (const id of ids) if (next.get(id)?.kind === "running") next.set(id, { kind: "failed", error });
          return next;
        });
      }
    },
    [backend, docId, notify],
  );

  /** Applies every proposal whose judge passed, one after another. */
  const applyPassed = useCallback(async () => {
    const targets = [...entriesRef.current].flatMap(([id, e]) =>
      e.kind === "ready" && !e.busy && e.proposal.judge?.passed ? [{ id, proposal: e.proposal }] : [],
    );
    if (targets.length === 0) return;
    let applied = 0;
    setApplyingAll({ done: 0, total: targets.length });
    for (const [i, t] of targets.entries()) {
      if (await apply(t.id, t.proposal, null, false)) applied++;
      setApplyingAll({ done: i + 1, total: targets.length });
    }
    setApplyingAll(null);
    const failed = targets.length - applied;
    notify(failed ? `已应用 ${applied} 条达标修改，${failed} 条未能应用` : `已应用 ${applied} 条达标修改`, failed > 0 && applied === 0);
  }, [apply, notify]);

  const closeBatch = useCallback(() => setBatch(null), []);

  return { entries, batch, applyingAll, fix, apply, reject, runBatch, applyPassed, closeBatch };
}
