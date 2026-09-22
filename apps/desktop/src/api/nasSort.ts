import { invoke } from "@tauri-apps/api/core";
import type { MatchStatus } from "@cinemantis/shared-types";

/** 振り分け判定の結果。ready 以外は移動されない。 */
export type NasSortStatus =
  | "ready"
  | "no_reading"
  | "no_country"
  | "dest_exists"
  | "file_missing"
  | "same_path";

export interface NasSortPlanRow {
  work_id: number;
  file_id: number;
  title: string;
  year: number | null;
  reading: string | null;
  country_type: string;
  file_size: number | null;
  source_path: string;
  dest_path: string | null;
  dest_display: string | null;
  status: NasSortStatus;
  note: string | null;
  match_status: MatchStatus;
}

export interface NasSortPlan {
  rows: NasSortPlanRow[];
  /** 取り込み元フォルダを含む登録済みソースのルート。未登録なら null */
  source_root: string | null;
}

export interface NasSortError {
  work_id: number;
  title: string;
  message: string;
}

export interface NasSortResult {
  moved: number;
  failed: number;
  skipped: number;
  errors: NasSortError[];
  /** 移動先を含むソースが無かったため新しく登録したソースのルート */
  registered_sources: string[];
}

/** 取り込み元フォルダ（サブフォルダを含む）の振り分け計画を取得する（移動はしない） */
export async function planNasSort(folder: string): Promise<NasSortPlan> {
  return invoke<NasSortPlan>("plan_nas_sort", { folder });
}

/**
 * 選択した作品を NAS へ移動する。
 * 移動先はバックエンドで再計算されるので work_id だけ渡す。
 */
export async function executeNasSort(
  folder: string,
  workIds: number[],
): Promise<NasSortResult> {
  return invoke<NasSortResult>("execute_nas_sort", { folder, workIds });
}
