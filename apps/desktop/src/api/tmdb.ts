import { invoke } from "@tauri-apps/api/core";

export interface TmdbCandidate {
  tmdb_id: number;
  media_type: "movie" | "tv";
  title: string;
  original_title: string | null;
  year: number | null;
  poster_path: string | null;    // TMDb 相対パス (/xxx.jpg)
  overview: string | null;
  confidence: number;
  reasons: string[];
}

export interface AutoMatchResult {
  work_id: number;
  matched: boolean;
  status: "auto" | "manual" | "locked" | "unmatched";
  confidence: number;
  tmdb_id: number | null;
  title: string | null;
  poster_local_path: string | null;
}

export interface MetadataBatchProgress {
  source_id: number | null;
  processed: number;
  total: number;
  matched: number;
  skipped: number;
  failed: number;
}

export interface MetadataUpdatedEvent {
  work_id: number;
  status: string;
  poster_local_path: string | null;
  title: string;
  year: number | null;
}

export function tmdbPosterUrl(path: string): string {
  return `https://image.tmdb.org/t/p/w300${path}`;
}

export async function searchTmdbCandidates(
  workId: number,
  queryOverride?: string | null,
  mediaTypeHint?: string | null,
): Promise<TmdbCandidate[]> {
  return invoke<TmdbCandidate[]>("search_tmdb_candidates", {
    work_id: workId,
    query_override: queryOverride ?? null,
    media_type_hint: mediaTypeHint ?? null,
  });
}

export async function autoMatchWork(workId: number): Promise<AutoMatchResult> {
  return invoke<AutoMatchResult>("auto_match_work", { work_id: workId });
}

export async function autoMatchSource(
  sourceId: number | null = null
): Promise<MetadataBatchProgress> {
  return invoke<MetadataBatchProgress>("auto_match_source", {
    source_id: sourceId,
  });
}

export async function applyTmdbMatch(
  workId: number,
  tmdbId: number,
  mediaType: string,
  lock: boolean
): Promise<AutoMatchResult> {
  return invoke<AutoMatchResult>("apply_tmdb_match", {
    work_id: workId,
    tmdb_id: tmdbId,
    media_type: mediaType,
    lock,
  });
}

export async function refreshTmdbMetadata(
  workId: number
): Promise<AutoMatchResult> {
  return invoke<AutoMatchResult>("refresh_tmdb_metadata", { work_id: workId });
}

export async function clearTmdbMatch(workId: number): Promise<void> {
  return invoke("clear_tmdb_match", { work_id: workId });
}

export async function unlockTmdbMatch(workId: number): Promise<void> {
  return invoke("unlock_tmdb_match", { work_id: workId });
}
