import { invoke } from "@tauri-apps/api/core";

/** 埋め込みメタデータの取得状況 */
export interface TagCoverage {
  files: number;
  with_tags: number;
  captured_at_scan: number;
  provider_known: number;
  pipeline_known: number;
  unknown: number;
  suspicious: number;
}

export interface TagBackfillProgress {
  processed: number;
  total: number;
  with_tags: number;
  failed: number;
  current_file: string | null;
}

export interface TagBackfillResult {
  processed: number;
  total: number;
  with_tags: number;
  failed: number;
  /** まだ読んでいないファイル数。0 でなければ続きを実行できる */
  remaining: number;
  /** タグと既存照合の食い違いで作ったレビュー課題の数 */
  conflicts_created: number;
}

export async function containerTagCoverage(): Promise<TagCoverage> {
  return invoke<TagCoverage>("container_tag_coverage");
}

/**
 * 動画ファイルの埋め込みメタデータを読み込む（ユーザー操作でのみ実行する）。
 * limit を指定すると、その件数だけ処理して止まる（お試し実行）。
 */
export async function backfillContainerTags(
  limit?: number,
  concurrency?: number,
): Promise<TagBackfillResult> {
  return invoke<TagBackfillResult>("backfill_container_tags", { limit, concurrency });
}
