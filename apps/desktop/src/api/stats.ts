import { invoke } from "@tauri-apps/api/core";

export interface UserStatsRow {
  work_id: number;
  user_rating: number | null;
  play_count: number;
  last_played_at: string | null;
  resume_position_sec: number | null;
  is_favorite: boolean;
  watch_status: string;
  personal_note: string | null;
}

export interface UpdateStatsPayload {
  work_id: number;
  user_rating?: number | null;
  watch_status?: string | null;
  is_favorite?: boolean | null;
  personal_note?: string | null;
  resume_position_sec?: number | null;
}

export async function getUserStats(workId: number): Promise<UserStatsRow | null> {
  return invoke<UserStatsRow | null>("get_user_stats", { workId });
}

export async function updateUserStats(payload: UpdateStatsPayload): Promise<void> {
  return invoke("update_user_stats", { payload });
}

/** Record a play event: increments play_count, sets last_played_at */
export async function recordPlay(workId: number): Promise<void> {
  return invoke("record_play", { workId });
}
