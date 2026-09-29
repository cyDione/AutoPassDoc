import { useCallback, useEffect, useRef, useState } from "react";
import type { Backend } from "../api";
import type { AuthorView, DocState, EditOutcome, OpenedDoc } from "../types";
import { errorMessage } from "../util";

export type Notify = (text: string, error?: boolean) => void;

/**
 * Runs a command that changes the document. Commands run one at a time, and a
 * returned outcome replaces the summary and the undo state.
 */
export type RunEdit = <T extends EditOutcome | null>(run: () => Promise<T>) => Promise<T>;

/** The open document and everything that edits it. */
export function useDocumentSession(backend: Backend | null, notify: Notify) {
  const [doc, setDoc] = useState<OpenedDoc | null>(null);
  const [docState, setDocState] = useState<DocState | null>(null);
  /** Bumped on every edit so the document view refetches its blocks. */
  const [version, setVersion] = useState(0);
  const [authors, setAuthors] = useState<AuthorView[]>([]);
  const [loading, setLoading] = useState(false);
  const docRef = useRef(doc);
  const queue = useRef<Promise<unknown>>(Promise.resolve());

  useEffect(() => {
    docRef.current = doc;
  }, [doc]);

  const enqueue = useCallback(<T,>(run: () => Promise<T>): Promise<T> => {
    const result = queue.current.then(run);
    queue.current = result.catch(() => {});
    return result;
  }, []);

  const applyOutcome = useCallback((outcome: EditOutcome) => {
    setDoc((d) => d && { ...d, summary: outcome.summary });
    setDocState(outcome.state);
    setVersion((v) => v + 1);
  }, []);

  const edit: RunEdit = useCallback(
    <T extends EditOutcome | null>(run: () => Promise<T>) => {
      const docId = docRef.current?.docId;
      return enqueue(run).then((outcome) => {
        if (outcome && docRef.current?.docId === docId) applyOutcome(outcome);
        return outcome;
      });
    },
    [enqueue, applyOutcome],
  );

  const refreshAuthors = useCallback(
    (docId: number) => {
      backend
        ?.documentAuthors(docId)
        .then((list) => docRef.current?.docId === docId && setAuthors(list))
        .catch(() => {});
    },
    [backend],
  );

  const load = useCallback(
    async (open: () => Promise<OpenedDoc | null>): Promise<boolean> => {
      if (!backend) return false;
      setLoading(true);
      try {
        const next = await open();
        if (!next) return false;
        const previous = docRef.current;
        if (previous) void backend.close(previous.docId);
        docRef.current = next;
        setDoc(next);
        setDocState(null);
        setAuthors([]);
        backend
          .docState(next.docId)
          .then((s) => docRef.current?.docId === next.docId && setDocState(s))
          .catch(() => {});
        refreshAuthors(next.docId);
        return true;
      } catch (e) {
        notify(`无法打开文档：${errorMessage(e)}`, true);
        return false;
      } finally {
        setLoading(false);
      }
    },
    [backend, notify, refreshAuthors],
  );

  const undoOrRedo = useCallback(
    async (which: "undo" | "redo") => {
      const current = docRef.current;
      if (!backend || !current) return;
      try {
        await edit(() => backend[which](current.docId));
        // Undoing a signature change renames authors back.
        refreshAuthors(current.docId);
      } catch (e) {
        notify(`${which === "undo" ? "撤销" : "重做"}失败：${errorMessage(e)}`, true);
      }
    },
    [backend, edit, notify, refreshAuthors],
  );
  const undo = useCallback(() => undoOrRedo("undo"), [undoOrRedo]);
  const redo = useCallback(() => undoOrRedo("redo"), [undoOrRedo]);

  const saveAs = useCallback(async () => {
    const current = docRef.current;
    if (!backend || !current) return;
    try {
      const path = await enqueue(() => backend.saveAs(current.docId));
      if (!path) return;
      notify(`已另存为 ${path}`);
      setDocState(await backend.docState(current.docId));
    } catch (e) {
      notify(`保存失败：${errorMessage(e)}`, true);
    }
  }, [backend, enqueue, notify]);

  const save = useCallback(async () => {
    const current = docRef.current;
    if (!backend || !current) return;
    if (!docState?.savedPath) return saveAs();
    try {
      const state = await enqueue(() => backend.save(current.docId));
      setDocState(state);
      notify(`已保存到 ${state.savedPath ?? docState.savedPath}`);
    } catch (e) {
      notify(`保存失败：${errorMessage(e)}`, true);
    }
  }, [backend, docState, enqueue, notify, saveAs]);

  return { doc, docState, version, authors, setAuthors, loading, load, edit, undo, redo, save, saveAs };
}
