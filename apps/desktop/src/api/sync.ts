import { invoke } from "@tauri-apps/api/core";

export interface SyncOutboxRow {
  id: number;
  action_type: string;
  source_id: number | null;
  work_id: number | null;
  work_title: string | null;
  target_path: string;
  status: "pending" | "running" | "done" | "failed" | "cancelled";
  attempts: number;
  error_message: string | null;
  created_at: string;
  updated_at: string;
  processed_at: string | null;
}

export interface SyncProcessResult {
  processed: number;
  deleted: number;
  already_missing: number;
  skipped_offline: number;
  failed: number;
}

export async function listSyncOutbox(): Promise<SyncOutboxRow[]> {
  return invoke<SyncOutboxRow[]>("list_sync_outbox");
}

export async function processSyncOutbox(sourceId?: number | null): Promise<SyncProcessResult> {
  return invoke<SyncProcessResult>("process_sync_outbox", { sourceId: sourceId ?? null });
}
