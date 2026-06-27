import { invoke } from "@tauri-apps/api/core";
import type { WorkSummary, SortField, SortOrder } from "@cinemantis/shared-types";

interface WorkSummaryRow {
  id: number;
  title: string;
  year: number | null;
  release_year: number | null;
  work_type: string;
  media_category: string;
  country_type: string;
  reading: string | null;
  genre_text: string | null;
  date_added: string | null;
  last_watched_at: string | null;
  storage_path: string | null;
  file_size: number | null;
  poster_path: string | null;
  user_rating: number | null;
  my_rating: number | null;
  play_count: number;
  watch_status: string;
  watched_status: string;
  is_favorite: boolean;
  runtime_sec: number | null;
  resume_position_sec: number | null;
  external_rating: number | null;
  match_status: string;
}

interface WorkDetailRow {
  id: number;
  title: string;
  original_title: string | null;
  year: number | null;
  release_year: number | null;
  work_type: string;
  media_category: string;
  country_type: string;
  reading: string | null;
  synopsis: string | null;
  runtime_sec: number | null;
  genres_json: string | null;
  genre_text: string | null;
  poster_path: string | null;
  thumb_path: string | null;
  external_rating: number | null;
  external_rating_source: string | null;
  tmdb_id: number | null;
  tmdb_media_type: string | null;
  imdb_id: string | null;
  match_status: string;
  match_confidence: number | null;
  release_date: string | null;
  date_added: string | null;
  file_size: number | null;
  user_rating: number | null;
  my_rating: number | null;
  play_count: number;
  last_played_at: string | null;
  last_watched_at: string | null;
  resume_position_sec: number | null;
  is_favorite: boolean;
  watch_status: string;
  watched_status: string;
  personal_note: string | null;
}

export interface WorkDetail extends WorkDetailRow {
  genres: string[];
}

export interface FilterOptions {
  genres: string[];
  countries: string[];
  country_types: string[];
  media_categories: string[];
  year_min: number | null;
  year_max: number | null;
}

function toSummary(r: WorkSummaryRow): WorkSummary {
  return {
    id: r.id,
    title: r.title,
    year: r.year,
    releaseYear: r.release_year,
    workType: r.work_type as WorkSummary["workType"],
    mediaCategory: r.media_category,
    countryType: r.country_type,
    reading: r.reading,
    genreText: r.genre_text,
    dateAdded: r.date_added,
    lastWatchedAt: r.last_watched_at,
    storagePath: r.storage_path,
    fileSize: r.file_size,
    posterPath: r.poster_path,
    userRating: r.user_rating,
    myRating: r.my_rating,
    playCount: r.play_count,
    watchStatus: r.watch_status as WorkSummary["watchStatus"],
    watchedStatus: r.watched_status as WorkSummary["watchedStatus"],
    isFavorite: r.is_favorite,
    runtimeSec: r.runtime_sec,
    resumePositionSec: r.resume_position_sec,
    externalRating: r.external_rating,
    matchStatus: r.match_status as WorkSummary["matchStatus"],
  };
}

export interface ListWorksParams {
  workType?: string | null;
  watchStatus?: string | null;
  matchStatus?: string | null;
  query?: string | null;
  // 拡張フィルタ
  yearFrom?: number | null;
  yearTo?: number | null;
  genre?: string | null;
  country?: string | null;
  countryType?: string | null;
  mediaCategory?: string | null;
  dateAddedFrom?: string | null;
  dateAddedTo?: string | null;
  personId?: number | null;
  seriesId?: number | null;
  minUserRating?: number | null;
  isFavorite?: boolean | null;
  tagIds?: number[];
  minPlayCount?: number | null;
  // ソート
  sortField?: SortField;
  sortOrder?: SortOrder;
  // 要確認フィルタ (AttentionCenterScreen 専用)
  attentionFilter?: string | null;
  unorganizedOnly?: boolean | null;
}

export interface UpdateWorkLibraryPayload {
  work_id: number;
  title?: string | null;
  reading?: string | null;
  country_type?: string | null;
  media_category?: string | null;
  release_year?: number | null;
  genre_text?: string | null;
}

export async function listWorks(params: ListWorksParams): Promise<WorkSummary[]> {
  const tagIdsJson =
    params.tagIds && params.tagIds.length > 0
      ? JSON.stringify(params.tagIds)
      : null;

  const rows = await invoke<WorkSummaryRow[]>("list_works", {
    workType:       params.workType      ?? null,
    watchStatus:    params.watchStatus   ?? null,
    matchStatus:    params.matchStatus   ?? null,
    query:          params.query         || null,
    yearFrom:       params.yearFrom      ?? null,
    yearTo:         params.yearTo        ?? null,
    genre:          params.genre         ?? null,
    country:        params.country       ?? null,
    countryType:    params.countryType   ?? null,
    mediaCategory:  params.mediaCategory ?? null,
    dateAddedFrom:  params.dateAddedFrom ?? null,
    dateAddedTo:    params.dateAddedTo   ?? null,
    personId:       params.personId      ?? null,
    seriesId:       params.seriesId      ?? null,
    minUserRating:  params.minUserRating ?? null,
    isFavorite:     params.isFavorite    ?? null,
    tagIdsJson:     tagIdsJson,
    minPlayCount:   params.minPlayCount  ?? null,
    sortField:      params.sortField     ?? "title",
    sortOrder:      params.sortOrder     ?? "asc",
    attentionFilter: params.attentionFilter ?? null,
    unorganizedOnly: params.unorganizedOnly ?? null,
  });
  return rows.map(toSummary);
}

export async function getWork(workId: number): Promise<WorkDetail | null> {
  const row = await invoke<WorkDetailRow | null>("get_work", { workId });
  if (!row) return null;
  const genresSource = row.genre_text ?? row.genres_json;
  let genres: string[] = [];
  if (genresSource) {
    try {
      const parsed = JSON.parse(genresSource);
      genres = Array.isArray(parsed) ? parsed : [];
    } catch {
      genres = genresSource.split(/[,\u3001/]/).map((g) => g.trim()).filter(Boolean);
    }
  }
  return {
    ...row,
    genres,
  };
}

export async function getFilterOptions(): Promise<FilterOptions> {
  return invoke<FilterOptions>("get_filter_options");
}

export async function updateWorkLibraryFields(
  payload: UpdateWorkLibraryPayload,
): Promise<void> {
  return invoke("update_work_library_fields", { payload });
}

export interface DeleteWorkFilesResult {
  removed_files: number;
  queued_files: number;
  missing_files: number;
}

export async function deleteWorkFiles(workId: number): Promise<DeleteWorkFilesResult> {
  return invoke<DeleteWorkFilesResult>("delete_work_files", { workId });
}
