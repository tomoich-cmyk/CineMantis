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

export function useScanProgress() {
  const [progress, setProgress] = useState<ScanProgress | null>(null);

  useEffect(() => {
    let unlisten: (() => void) | undefined;

    listen<ScanProgress>("scan:progress", (event) => {
      setProgress(event.payload);
      if (event.payload.phase === "done") {
        // Auto-clear after 3 seconds
        setTimeout(() => setProgress(null), 3000);
      }
    }).then((fn) => {
      unlisten = fn;
    });

    return () => unlisten?.();
  }, []);

  return progress;
}
