import { invoke } from "@tauri-apps/api/core";
import type { Source } from "@cinemantis/shared-types";

export interface SourceRow {
  id: number;
  name: string;
  root_path: string;
  source_type: string;
  is_enabled: boolean;
  status: string;
  last_scan_at: string | null;
  last_seen_at: string | null;
}

function toSource(r: SourceRow): Source {
  return {
    id: r.id,
    name: r.name,
    rootPath: r.root_path,
    sourceType: r.source_type as Source["sourceType"],
    isEnabled: r.is_enabled,
    status: r.status as Source["status"],
    lastScanAt: r.last_scan_at,
    lastSeenAt: r.last_seen_at,
  };
}

export async function listSources(): Promise<Source[]> {
  const rows = await invoke<SourceRow[]>("list_sources");
  return rows.map(toSource);
}

export async function addSource(payload: {
  name: string;
  root_path: string;
  source_type: string;
}): Promise<number> {
  return invoke<number>("add_source", { payload });
}

export async function updateSourceStatus(
  sourceId: number,
  status: string
): Promise<void> {
  return invoke("update_source_status", { source_id: sourceId, status });
}

/** Trigger a full folder scan for a source */
export async function scanSource(sourceId: number): Promise<void> {
  return invoke("scan_source", { source_id: sourceId });
}
