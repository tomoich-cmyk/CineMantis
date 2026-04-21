// ─── Enums / Union Types ────────────────────────────────────────────────────

export type WorkType = "movie" | "drama" | "ova" | "special" | "other";

export type WatchStatus = "unwatched" | "watching" | "watched" | "skipped";

export type MatchStatus = "unmatched" | "pending" | "matched" | "locked";

export type SourceType = "nas" | "external_hdd" | "local";

export type SourceStatus = "online" | "offline" | "error";

export type RoleType = "director" | "screenplay" | "cast" | "producer" | "music" | "other";

export type SeriesType = "movie_series" | "drama_series" | "collection";

export type AvailabilityStatus = "available" | "missing" | "offline";

// ─── Source ─────────────────────────────────────────────────────────────────

/** ソースに登録されているライブラリ種別 */
export type SourceMediaKind = "movie" | "tv" | "unknown";

export interface Source {
  id: number;
  name: string;
  rootPath: string;
  sourceType: SourceType;
  /** "movie" = 映画, "tv" = ドラマ, "unknown" = 自動判定 */
  mediaKind: SourceMediaKind;
  isEnabled: boolean;
  status: SourceStatus;
  lastScanAt: string | null;
  lastSeenAt: string | null;
}

// ─── File ────────────────────────────────────────────────────────────────────

export interface MediaFile {
  id: number;
  sourceId: number;
  filePath: string;
  fileName: string;
  extension: string;
  fileSize: number | null;
  mtime: string | null;
  ctime: string | null;
  durationSec: number | null;
  width: number | null;
  height: number | null;
  videoCodec: string | null;
  audioCodec: string | null;
  container: string | null;
  hashPartial: string | null;
  availabilityStatus: AvailabilityStatus;
  createdAt: string;
  updatedAt: string;
}

// ─── Work ────────────────────────────────────────────────────────────────────

export interface Work {
  id: number;
  workType: WorkType;
  title: string;
  originalTitle: string | null;
  sortTitle: string | null;
  year: number | null;
  country: string | null;
  synopsis: string | null;
  runtimeSec: number | null;
  genresJson: string[] | null;
  posterPath: string | null;
  thumbPath: string | null;
  externalRating: number | null;
  externalRatingSource: string | null;
  tmdbId: number | null;
  imdbId: string | null;
  matchStatus: MatchStatus;
  matchConfidence: number | null;
  releaseDate: string | null;
  createdAt: string;
  updatedAt: string;
}

// ─── WorkPart ────────────────────────────────────────────────────────────────

export interface WorkPart {
  id: number;
  workId: number;
  fileId: number;
  partNo: number;
  partLabel: string | null;
  startSec: number | null;
  endSec: number | null;
  playOrder: number;
}

// ─── UserStats ───────────────────────────────────────────────────────────────

export interface UserStats {
  workId: number;
  userRating: number | null;   // 1–5
  playCount: number;
  lastPlayedAt: string | null;
  resumePositionSec: number | null;
  isFavorite: boolean;
  watchStatus: WatchStatus;
  personalNote: string | null;
  updatedAt: string;
}

// ─── Tag ─────────────────────────────────────────────────────────────────────

export interface Tag {
  id: number;
  name: string;
  color: string | null;
  tagType: "user" | "smart" | "system";
}

// ─── Person ──────────────────────────────────────────────────────────────────

export interface Person {
  id: number;
  name: string;
  originalName: string | null;
  sortName: string | null;
  personTypeHint: RoleType | null;
  tmdbPersonId: number | null;
  thumbPath: string | null;
  createdAt: string;
  updatedAt: string;
}

export interface WorkPerson {
  id: number;
  workId: number;
  personId: number;
  roleType: RoleType;
  billingOrder: number | null;
  characterName: string | null;
}

// ─── Series ──────────────────────────────────────────────────────────────────

export interface Series {
  id: number;
  name: string;
  originalName: string | null;
  seriesType: SeriesType;
  tmdbId: number | null;
  externalSource: string | null;
  posterPath: string | null;
  sortModeDefault: string | null;
  createdAt: string;
  updatedAt: string;
}

export interface SeriesItem {
  id: number;
  seriesId: number;
  workId: number;
  seasonNo: number | null;
  episodeNo: number | null;
  seriesOrder: number | null;
  displayGroup: string | null;
  isPrimarySeries: boolean;
}
