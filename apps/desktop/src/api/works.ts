import { invoke } from "@tauri-apps/api/core";
import type { WorkSummary, SortField, SortOrder } from "@cinemantis/shared-types";

interface WorkSummaryRow {
  id: number;
  title: string;
  year: number | null;
  work_type: string;
  poster_path: string | null;
  user_rating: number | null;
  play_count: number;
  watch_status: string;
  is_favorite: boolean;
  runtime_sec: number | null;
  external_rating: number | null;
  match_status: string;
}

interface WorkDetailRow {
  id: number;
  title: string;
  original_title: string | null;
  year: number | null;
  work_type: string;
  synopsis: string | null;
  runtime_sec: number | null;
  genres_json: string | null;
  poster_path: string | null;
  thumb_path: string | null;
  external_rating: number | null;
  external_rating_source: string | null;
  tmdb_id: number | null;
  imdb_id: string | null;
  match_status: string;
  release_date: string | null;
  user_rating: number | null;
  play_count: number;
  last_played_at: string | null;
  resume_position_sec: number | null;
  is_favorite: boolean;
  watch_status: string;
  personal_note: string | null;
}

export interface WorkDetail extends WorkDetailRow {
  genres: string[];
}

function toSummary(r: WorkSummaryRow): WorkSummary {
  return {
    id: r.id,
    title: r.title,
    year: r.year,
    workType: r.work_type as WorkSummary["workType"],
    posterPath: r.poster_path,
    userRating: r.user_rating,
    playCount: r.play_count,
    watchStatus: r.watch_status as WorkSummary["watchStatus"],
    isFavorite: r.is_favorite,
    runtimeSec: r.runtime_sec,
    externalRating: r.external_rating,
    matchStatus: r.match_status as WorkSummary["matchStatus"],
  };
}

export interface ListWorksParams {
  workType?: string | null;
  watchStatus?: string | null;
  query?: string | null;
  sortField?: SortField;
  sortOrder?: SortOrder;
}

export async function listWorks(params: ListWorksParams): Promise<WorkSummary[]> {
  const rows = await invoke<WorkSummaryRow[]>("list_works", {
    work_type: params.workType ?? null,
    watch_status: params.watchStatus ?? null,
    query: params.query || null,
    sort_field: params.sortField ?? "title",
    sort_order: params.sortOrder ?? "asc",
  });
  return rows.map(toSummary);
}

export async function getWork(workId: number): Promise<WorkDetail | null> {
  const row = await invoke<WorkDetailRow | null>("get_work", { work_id: workId });
  if (!row) return null;
  return {
    ...row,
    genres: row.genres_json ? JSON.parse(row.genres_json) : [],
  };
}
