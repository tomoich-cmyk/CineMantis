import type { MatchStatus, WatchStatus, WorkType } from "./domain";

// ─── Navigation ──────────────────────────────────────────────────────────────

export type NavSection =
  | "all-movies"
  | "all-drama"
  | "series"
  | "persons"
  | "tags"
  | "unwatched"
  | "watching"
  | "recently-added"
  | "high-rated"
  | "sources"
  | "settings";

// ─── Sort ────────────────────────────────────────────────────────────────────

export type SortField =
  | "title"
  | "year"
  | "user_rating"
  | "external_rating"
  | "play_count"
  | "last_played_at"
  | "created_at"
  | "updated_at"
  | "runtime_sec"
  | "release_date";

export type SortOrder = "asc" | "desc";

// ─── View models ─────────────────────────────────────────────────────────────

/** Lightweight summary used in grid/list views */
export interface WorkSummary {
  id: number;
  title: string;
  year: number | null;
  workType: WorkType;
  posterPath: string | null;
  userRating: number | null;
  playCount: number;
  watchStatus: WatchStatus;
  isFavorite: boolean;
  runtimeSec: number | null;
  externalRating: number | null;
  matchStatus: MatchStatus;
}
