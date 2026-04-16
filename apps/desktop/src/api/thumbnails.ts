import { invoke } from "@tauri-apps/api/core";

export interface ThumbBatchProgress {
  total: number;
  done: number;
  failed: number;
}

/** 単体サムネイル生成。生成済みなら即パスを返す */
export async function generateThumbnail(workId: number): Promise<string> {
  return invoke<string>("generate_thumbnail", { workId });
}

/**
 * ソース単位のバッチ生成。
 * source_id が null なら全ソース対象。
 */
export async function generateThumbnailsBatch(
  sourceId: number | null = null
): Promise<ThumbBatchProgress> {
  return invoke<ThumbBatchProgress>("generate_thumbnails_batch", { sourceId });
}

/** thumb_path を手動で上書き */
export async function setCustomThumb(
  workId: number,
  path: string
): Promise<void> {
  return invoke("set_custom_thumb", { workId, path });
}
