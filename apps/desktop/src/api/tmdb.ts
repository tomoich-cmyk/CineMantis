import { invoke } from "@tauri-apps/api/core";
import type { MatchStatus } from "@cinemantis/shared-types";

export interface TmdbCandidate {
  tmdb_id: number;
  media_type: "movie" | "tv";
  title: string;
  original_title: string | null;
  year: number | null;
  poster_path: string | null;    // TMDb 相対パス (/xxx.jpg)
  overview: string | null;
  /**
   * 候補一覧での並べ替え用スコア。
   * PR2.5 以降はファイル名由来と埋め込みメタデータ由来の検索を統合した値で、
   * 自動確定（production AUTO）の確信度ではない。
   * 自動照合の判定は AutoMatchResult.decision / reasons を見ること。
   */
  confidence: number;
  reasons: string[];
}

/** 自動照合（rules-safe）の判定 */
export type MatchDecision = "AUTO" | "REVIEW" | "UNRESOLVED";

export interface AutoMatchResult {
  work_id: number;
  matched: boolean;
  status: MatchStatus;
  confidence: number;
  tmdb_id: number | null;
  title: string | null;
  poster_local_path: string | null;
  /** 自動照合のときだけ入る */
  decision?: MatchDecision;
  /** AUTO にしなかった理由コード */
  reasons?: string[];
}

export interface MetadataBatchProgress {
  source_id: number | null;
  processed: number;
  total: number;
  matched: number;
  skipped: number;
  failed: number;
  /** デバッグ用: 最後に発生したエラーや候補なしの理由 */
  last_error?: string;
}

export interface MetadataUpdatedEvent {
  work_id: number;
  status: string;
  poster_local_path: string | null;
  title: string;
  year: number | null;
}

export function tmdbPosterUrl(path: string): string {
  return `https://image.tmdb.org/t/p/w500${path}`;
}

export async function searchTmdbCandidates(
  workId: number,
  queryOverride?: string | null,
  mediaTypeHint?: string | null,
): Promise<TmdbCandidate[]> {
  return invoke<TmdbCandidate[]>("search_tmdb_candidates", {
    workId,
    queryOverride: queryOverride ?? null,
    mediaTypeHint: mediaTypeHint ?? null,
  });
}

export async function autoMatchWork(workId: number): Promise<AutoMatchResult> {
  return invoke<AutoMatchResult>("auto_match_work", { workId });
}

export async function autoMatchSource(
  sourceId: number | null = null
): Promise<MetadataBatchProgress> {
  return invoke<MetadataBatchProgress>("auto_match_source", { sourceId });
}

export async function testTmdbApi(): Promise<string> {
  return invoke<string>("test_tmdb_api");
}

/** 人がどう確定したか（照合履歴のラベルに残る） */
export type ApplyMethod = "manual_apply" | "manual_direct_id";

export async function applyTmdbMatch(
  workId: number,
  tmdbId: number,
  mediaType: string,
  lock: boolean,
  method: ApplyMethod = "manual_apply"
): Promise<AutoMatchResult> {
  return invoke<AutoMatchResult>("apply_tmdb_match", {
    workId,
    tmdbId,
    mediaType,
    lock,
    method,
  });
}

export async function refreshTmdbMetadata(
  workId: number
): Promise<AutoMatchResult> {
  return invoke<AutoMatchResult>("refresh_tmdb_metadata", { workId });
}

export async function clearTmdbMatch(workId: number): Promise<void> {
  return invoke("clear_tmdb_match", { workId });
}

export async function unlockTmdbMatch(workId: number): Promise<void> {
  return invoke("unlock_tmdb_match", { workId });
}
