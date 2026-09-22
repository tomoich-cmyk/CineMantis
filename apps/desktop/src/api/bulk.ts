import { invoke } from "@tauri-apps/api/core";

export interface AttentionStats {
  unmatched: number;
  /** レビュー待ち（候補はあるが自動確定できなかった） */
  pending: number;
  no_poster: number;
  missing_meta: number;
  no_persons: number;
  file_missing: number;
  source_offline: number;
}

export async function getAttentionStats(): Promise<AttentionStats> {
  return invoke("get_attention_stats");
}

function toJson(ids: number[]): string {
  return JSON.stringify(ids);
}

export async function bulkSetWatchStatus(
  workIds: number[],
  status: "unwatched" | "watching" | "watched" | "skipped"
): Promise<void> {
  return invoke("bulk_set_watch_status", {
    workIdsJson: toJson(workIds),
    status,
  });
}

export async function bulkSetFavorite(
  workIds: number[],
  isFavorite: boolean
): Promise<void> {
  return invoke("bulk_set_favorite", {
    workIdsJson: toJson(workIds),
    isFavorite,
  });
}

export async function bulkAddTag(workIds: number[], tagId: number): Promise<void> {
  return invoke("bulk_add_tag", {
    workIdsJson: toJson(workIds),
    tagId,
  });
}

export async function bulkRemoveTag(workIds: number[], tagId: number): Promise<void> {
  return invoke("bulk_remove_tag", {
    workIdsJson: toJson(workIds),
    tagId,
  });
}

/**
 * locked: tmdb_id がある作品だけ固定 / matched: 固定を解除 /
 * unmatched: 照合を解除（固定中は対象外） / pending: レビュー待ちに戻す
 */
export type BulkMatchStatus = "locked" | "matched" | "unmatched" | "pending";

export interface BulkMatchStatusResult {
  /** 状態を変更した件数 */
  updated: number;
  /** 条件に合わず変更しなかった件数 */
  skipped: number;
}

export async function bulkSetMatchStatus(
  workIds: number[],
  status: BulkMatchStatus
): Promise<BulkMatchStatusResult> {
  return invoke("bulk_set_match_status", {
    workIdsJson: toJson(workIds),
    status,
  });
}
