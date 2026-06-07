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
  | "continue-watching"
  | "favorites"
  | "recently-added"
  | "recently-played"
  | "high-rated"
  | "unorganized"
  | "completed"
  | "stalled"
  | "needs-attention"
  | "sources"
  | "settings"
  | "backup"
  | "audit"
  | "about";

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
  | "date_added"
  | "last_watched_at"
  | "my_rating"
  | "watched_status"
  | "media_category"
  | "country_type"
  | "reading"
  | "release_year"
  | "runtime_sec"
  | "release_date";

export type SortOrder = "asc" | "desc";

// ─── View models ─────────────────────────────────────────────────────────────

/** Lightweight summary used in grid/list views */
export interface WorkSummary {
  id: number;
  title: string;
  year: number | null;
  releaseYear: number | null;
  workType: WorkType;
  mediaCategory: string;
  countryType: string;
  genreText: string | null;
  dateAdded: string | null;
  lastWatchedAt: string | null;
  storagePath: string | null;
  fileSize: number | null;
  posterPath: string | null;
  userRating: number | null;
  myRating: number | null;
  playCount: number;
  watchStatus: WatchStatus;
  watchedStatus: WatchStatus;
  isFavorite: boolean;
  runtimeSec: number | null;
  resumePositionSec: number | null;
  externalRating: number | null;
  matchStatus: MatchStatus;
}
