import { invoke } from "@tauri-apps/api/core";
import type { WorkSummary } from "@cinemantis/shared-types";

// ─── 型定義 ──────────────────────────────────────────────────────────────────

export type SeriesType = "movie_collection" | "tv_show" | "manual";

export interface SeriesSummary {
  id: number;
  title: string;
  series_type: SeriesType;
  tmdb_id: number | null;
  poster_path: string | null;
  work_count: number;
}

export interface SeriesDetail {
  id: number;
  title: string;
  series_type: SeriesType;
  tmdb_id: number | null;
  poster_path: string | null;
  overview: string | null;
}

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

// ─── Tauri invoke ラッパー ────────────────────────────────────────────────────

export async function listSeries(): Promise<SeriesSummary[]> {
  return invoke<SeriesSummary[]>("list_series");
}

export async function getSeriesDetail(seriesId: number): Promise<SeriesDetail | null> {
  return invoke<SeriesDetail | null>("get_series", { series_id: seriesId });
}

export async function getSeriesWorks(seriesId: number): Promise<WorkSummary[]> {
  const rows = await invoke<WorkSummaryRow[]>("get_series_works", {
    series_id: seriesId,
  });
  return rows.map((r) => ({
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
  }));
}

export async function createSeries(title: string, seriesType?: string): Promise<number> {
  return invoke<number>("create_series", {
    title,
    series_type: seriesType ?? null,
  });
}

export async function addToSeries(
  seriesId: number,
  workId: number,
  sortOrder?: number,
): Promise<void> {
  return invoke("add_to_series", {
    series_id: seriesId,
    work_id: workId,
    sort_order: sortOrder ?? null,
  });
}

export async function removeFromSeries(seriesId: number, workId: number): Promise<void> {
  return invoke("remove_from_series", { series_id: seriesId, work_id: workId });
}

export async function deleteSeries(seriesId: number): Promise<void> {
  return invoke("delete_series", { series_id: seriesId });
}
