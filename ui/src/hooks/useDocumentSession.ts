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
  const dirtyRef = useRef(false);
  const queue = useRef<Promise<unknown>>(Promise.resolve());

  useEffect(() => {
    docRef.current = doc;
  }, [doc]);
  useEffect(() => {
    dirtyRef.current = !!docState?.dirty;
  }, [docState]);

  /** Whether the open document's unsaved changes may be dropped. */
  const confirmDiscard = useCallback(async (): Promise<boolean> => {
    const current = docRef.current;
    if (!backend || !current || !dirtyRef.current) return true;
    return backend.confirm(
      `“${current.fileName}”有未保存的修改，继续将丢弃这些修改。`,
      "放弃未保存的修改？",
      "放弃修改",
    );
  }, [backend]);

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
        if (!(await confirmDiscard())) {
          void backend.close(next.docId);
          return false;
        }
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
    [backend, confirmDiscard, notify, refreshAuthors],
  );

  // Closing the window asks too.
  useEffect(() => backend?.onCloseRequested(confirmDiscard), [backend, confirmDiscard]);

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

  /** Resolves true once the document is written to disk. */
  const saveAs = useCallback(async (): Promise<boolean> => {
    const current = docRef.current;
    if (!backend || !current) return false;
    try {
      const path = await enqueue(() => backend.saveAs(current.docId));
      if (!path) return false;
      // Like Word, the title now names the copy that 保存 writes to.
      const fileName = path.split(/[\\/]/).pop() || current.fileName;
      if (docRef.current?.docId === current.docId) {
        docRef.current = { ...docRef.current, fileName };
        setDoc((d) => (d?.docId === current.docId ? { ...d, fileName } : d));
      }
      notify(`已另存为 ${path}`);
      const state = await backend.docState(current.docId);
      setDocState(state);
      dirtyRef.current = state.dirty;
      return true;
    } catch (e) {
      notify(`保存失败：${errorMessage(e)}`, true);
      return false;
    }
  }, [backend, enqueue, notify]);

  const save = useCallback(async (): Promise<boolean> => {
    const current = docRef.current;
    if (!backend || !current) return false;
    if (!docState?.savedPath) return saveAs();
    try {
      const state = await enqueue(() => backend.save(current.docId));
      setDocState(state);
      dirtyRef.current = state.dirty;
      notify(`已保存到 ${state.savedPath ?? docState.savedPath}`);
      return true;
    } catch (e) {
      notify(`保存失败：${errorMessage(e)}`, true);
      return false;
    }
  }, [backend, docState, enqueue, notify, saveAs]);

  const drop = useCallback(() => {
    const current = docRef.current;
    if (!backend || !current) return;
    void backend.close(current.docId);
    docRef.current = null;
    dirtyRef.current = false;
    setDoc(null);
    setDocState(null);
    setAuthors([]);
  }, [backend]);

  /** Closes the document, asking first whether to save unsaved changes. Resolves true once it is closed. */
  const close = useCallback(async (): Promise<boolean> => {
    const current = docRef.current;
    if (!backend || !current) return false;
    if (dirtyRef.current) {
      const choice = await backend.askSave(`“${current.fileName}”有未保存的修改，关闭前要保存吗？`, "关闭文档");
      if (choice === "cancel") return false;
      if (choice === "save" && !(await save())) return false;
    }
    // Edits still running would land on a closed document.
    await queue.current;
    if (docRef.current?.docId !== current.docId) return false;
    drop();
    return true;
  }, [backend, save, drop]);

  const saveAndClose = useCallback(async (): Promise<boolean> => {
    if (!(await save())) return false;
    return close();
  }, [save, close]);

  return { doc, docState, version, authors, setAuthors, loading, load, edit, undo, redo, save, saveAs, close, saveAndClose };
}
