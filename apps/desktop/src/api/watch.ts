import { invoke } from "@tauri-apps/api/core";

/** 作品のファイルを OS 既定アプリで開き、再生記録を更新する */
export async function openWorkFile(workId: number): Promise<void> {
  return invoke("open_work_file", { workId });
}

/** 視聴状態を直接セット */
export async function setWatchStatus(
  workId: number,
  status: "unwatched" | "watching" | "watched" | "skipped"
): Promise<void> {
  return invoke("set_watch_status", { workId, status });
}

/** 再生再開位置（秒）を保存 */
export async function updateResumePosition(
  workId: number,
  positionSec: number
): Promise<void> {
  return invoke("update_resume_position", { workId, positionSec });
}
