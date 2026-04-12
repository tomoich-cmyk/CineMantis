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
  return invoke<PersonSummary[]>("list_persons", { role_filter: roleFilter ?? null });
}

export async function getPerson(personId: number): Promise<PersonSummary | null> {
  return invoke<PersonSummary | null>("get_person", { person_id: personId });
}

export async function getWorkPersons(workId: number): Promise<WorkPerson[]> {
  return invoke<WorkPerson[]>("get_work_persons", { work_id: workId });
}

export async function getPersonWorks(
  personId: number,
  roleFilter?: string | null,
): Promise<WorkSummary[]> {
  const rows = await invoke<WorkSummaryRow[]>("get_person_works", {
    person_id: personId,
    role_filter: roleFilter ?? null,
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
