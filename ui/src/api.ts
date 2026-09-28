import type { BlockView, OpenedDoc, Summary } from "./types";

/** Everything the UI needs from the Rust side. */
export interface Backend {
  /** Shows a file picker and opens the chosen document; `null` if cancelled. */
  pickAndOpen(): Promise<OpenedDoc | null>;
  open(path: string): Promise<OpenedDoc>;
  blocks(docId: number, start: number, end: number): Promise<BlockView[]>;
  /** Asks where to save; returns the saved path, or `null` if cancelled. */
  saveAs(docId: number): Promise<string | null>;
  close(docId: number): Promise<void>;
  /** A document passed on the command line at startup, if any. */
  initialFile(): Promise<string | null>;
  imageUrl(docId: number, relId: string): string;
  /** Subscribes to files dropped on the window; returns an unsubscribe function. */
  onFileDrop(handler: (paths: string[]) => void, onHover: (hovering: boolean) => void): () => void;
}

const isTauri = typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;

async function tauriBackend(): Promise<Backend> {
  const { invoke, convertFileSrc } = await import("@tauri-apps/api/core");
  const dialog = await import("@tauri-apps/plugin-dialog");
  const { getCurrentWebview } = await import("@tauri-apps/api/webview");

  const open = (path: string) => invoke<OpenedDoc>("open_document", { path });

  return {
    open,
    async pickAndOpen() {
      const path = await dialog.open({
        multiple: false,
        directory: false,
        filters: [{ name: "Word 文档", extensions: ["docx"] }],
      });
      return path ? open(path) : null;
    },
    blocks: (docId, start, end) => invoke<BlockView[]>("get_blocks", { docId, start, end }),
    async saveAs(docId) {
      const defaultPath = await invoke<string>("suggested_save_path", { docId });
      const path = await dialog.save({
        defaultPath,
        filters: [{ name: "Word 文档", extensions: ["docx"] }],
      });
      if (!path) return null;
      await invoke("save_document_as", { docId, path });
      return path;
    },
    close: (docId) => invoke("close_document", { docId }),
    initialFile: () => invoke<string | null>("initial_file"),
    imageUrl: (docId, relId) => convertFileSrc(`${docId}/${relId}`, "apd"),
    onFileDrop(handler, onHover) {
      let unlisten: (() => void) | null = null;
      let cancelled = false;
      getCurrentWebview()
        .onDragDropEvent((event) => {
          const p = event.payload;
          if (p.type === "enter" || p.type === "over") onHover(true);
          else if (p.type === "leave") onHover(false);
          else if (p.type === "drop") {
            onHover(false);
            handler(p.paths);
          }
        })
        .then((fn) => (cancelled ? fn() : (unlisten = fn)));
      return () => {
        cancelled = true;
        unlisten?.();
      };
    },
  };
}

/**
 * Browser-only backend for developing the UI without Tauri: serves a
 * generated sample report exported by `pnpm demo-data`.
 */
function demoBackend(): Backend {
  let blocks: Promise<BlockView[]> | null = null;
  const loadBlocks = () => (blocks ??= fetch("/demo/blocks.json").then((r) => r.json()));
  const open = async (): Promise<OpenedDoc> => {
    const summary: Summary = await fetch("/demo/summary.json").then((r) => r.json());
    await loadBlocks();
    return { docId: 1, path: "示例/某市数字政府项目可研报告.docx", fileName: "某市数字政府项目可研报告.docx", summary };
  };
  return {
    open,
    pickAndOpen: open,
    blocks: async (_docId, start, end) => (await loadBlocks()).slice(start, end),
    async saveAs() {
      return "示例/某市数字政府项目可研报告_AutoPassDoc.docx";
    },
    async close() {},
    async initialFile() {
      return null;
    },
    imageUrl: (_docId, relId) => `/demo/${relId}.png`,
    onFileDrop: () => () => {},
  };
}

export const backend: Promise<Backend> = isTauri ? tauriBackend() : Promise.resolve(demoBackend());
