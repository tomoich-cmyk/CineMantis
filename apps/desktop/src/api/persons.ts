import { invoke } from "@tauri-apps/api/core";
import type { WorkSummary } from "@cinemantis/shared-types";

// ─── 型定義 ──────────────────────────────────────────────────────────────────

export type PersonRole = "director" | "writer" | "cast";

export interface PersonSummary {
  id: number;
  name: string;
  profile_path: string | null;
  work_count: number;
  roles: string;   // カンマ区切り: "director,cast" など
}

export interface WorkPerson {
  person_id: number;
  name: string;
  role: PersonRole;
  character_name: string | null;
  display_order: number;
}

export interface PersonsSyncResult {
  total: number;
  succeeded: number;
  failed: number;
  last_error: string | null;
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

export async function listPersons(roleFilter?: string | null): Promise<PersonSummary[]> {
  return invoke<PersonSummary[]>("list_persons", { roleFilter: roleFilter ?? null });
}

export async function getPerson(personId: number): Promise<PersonSummary | null> {
  return invoke<PersonSummary | null>("get_person", { personId });
}

export async function getWorkPersons(workId: number): Promise<WorkPerson[]> {
  return invoke<WorkPerson[]>("get_work_persons", { workId });
}

export async function syncMissingPersons(): Promise<PersonsSyncResult> {
  return invoke<PersonsSyncResult>("repair_fetch_all_persons");
}

export async function localizePersonNames(): Promise<PersonsSyncResult> {
  return invoke<PersonsSyncResult>("localize_person_names");
}

export async function getPersonWorks(
  personId: number,
  roleFilter?: string | null,
): Promise<WorkSummary[]> {
  const rows = await invoke<WorkSummaryRow[]>("get_person_works", {
    personId,
    roleFilter: roleFilter ?? null,
  });
  return rows.map((r) => ({
    id: r.id,
    title: r.title,
    year: r.year,
    releaseYear: r.year,
    workType: r.work_type as WorkSummary["workType"],
    mediaCategory: r.work_type,
      countryType: "unknown",
      reading: null,
      genreText: null,
    dateAdded: null,
    lastWatchedAt: null,
    storagePath: null,
    fileSize: null,
    posterPath: r.poster_path,
    userRating: r.user_rating,
    myRating: r.user_rating,
    playCount: r.play_count,
    watchStatus: r.watch_status as WorkSummary["watchStatus"],
    watchedStatus: r.watch_status as WorkSummary["watchedStatus"],
    isFavorite: r.is_favorite,
    runtimeSec: r.runtime_sec,
    externalRating: r.external_rating,
    matchStatus: r.match_status as WorkSummary["matchStatus"],
    resumePositionSec: null,
  }));
}
