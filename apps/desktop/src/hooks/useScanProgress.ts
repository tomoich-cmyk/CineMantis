import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";

export interface ScanProgress {
  source_id: number;
  phase: "walking" | "probing" | "done";
  total: number;
  done: number;
  current_file: string | null;
  new_files: number;
  updated_files: number;
}

export interface ThumbBatchProgress {
  total: number;
  done: number;
  failed: number;
}

export function useScanProgress() {
  const [progress, setProgress] = useState<ScanProgress | null>(null);

  useEffect(() => {
    let unlisten: (() => void) | undefined;

    listen<ScanProgress>("scan:progress", (event) => {
      setProgress(event.payload);
      if (event.payload.phase === "done") {
        setTimeout(() => setProgress(null), 3000);
      }
    }).then((fn) => {
      unlisten = fn;
    });

    return () => unlisten?.();
  }, []);

  return progress;
}

export function useThumbBatchProgress() {
  const [progress, setProgress] = useState<ThumbBatchProgress | null>(null);

  useEffect(() => {
    let unlisten: (() => void) | undefined;

    listen<ThumbBatchProgress>("thumb:batch_progress", (event) => {
      setProgress(event.payload);
      // 全完了 or 0件なら3秒後にクリア
      if (
        event.payload.total > 0 &&
        event.payload.done + event.payload.failed >= event.payload.total
      ) {
        setTimeout(() => setProgress(null), 3000);
      }
    }).then((fn) => {
      unlisten = fn;
    });

    return () => unlisten?.();
  }, []);

  return progress;
}
