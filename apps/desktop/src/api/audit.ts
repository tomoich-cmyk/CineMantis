import { invoke } from "@tauri-apps/api/core";

// ─── 型定義 ───────────────────────────────────────────────────────────────────

export interface DuplicateGroup {
  reason: "same_tmdb_id" | "same_title_year";
  key: string;
  workIds: number[];
  titles: string[];
  years: (number | null)[];
}

export interface IntegrityIssue {
  workId: number;
  title: string;
  year: number | null;
  issues: IssueCode[];
}

export type IssueCode =
  | "unmatched"
  | "missing_meta"
  | "no_persons"
  | "watching_no_resume"
  | "watched_no_playcount"
  | "tmdb_no_overview"
  | "no_parts";

export interface IntegrityReport {
  issues: IntegrityIssue[];
  totalWorks: number;
  totalIssues: number;
}

// ─── camelCase 変換 ───────────────────────────────────────────────────────────

function toDuplicateGroup(raw: {
  reason: string;
  key: string;
  work_ids: number[];
  titles: string[];
  years: (number | null)[];
}): DuplicateGroup {
  return {
    reason: raw.reason as DuplicateGroup["reason"],
    key: raw.key,
    workIds: raw.work_ids,
    titles: raw.titles,
    years: raw.years,
  };
}

function toIntegrityIssue(raw: {
  work_id: number;
  title: string;
  year: number | null;
  issues: string[];
}): IntegrityIssue {
  return {
    workId: raw.work_id,
    title: raw.title,
    year: raw.year,
    issues: raw.issues as IssueCode[],
  };
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
function toIntegrityReport(raw: any): IntegrityReport {
  return {
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    issues: (raw.issues ?? []).map((i: any) => toIntegrityIssue(i)),
    totalWorks: raw.total_works,
    totalIssues: raw.total_issues,
  };
}

// ─── API 関数 ─────────────────────────────────────────────────────────────────

// eslint-disable-next-line @typescript-eslint/no-explicit-any
export async function getDuplicateGroups(): Promise<DuplicateGroup[]> {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const raw: any[] = await invoke("get_duplicate_groups");
  return raw.map(toDuplicateGroup);
}

export async function getIntegrityReport(): Promise<IntegrityReport> {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const raw: any = await invoke("get_integrity_report");
  return toIntegrityReport(raw);
}

export async function deleteWork(workId: number): Promise<void> {
  await invoke("delete_work", { workId });
}

export async function repairFetchPersons(workId: number): Promise<void> {
  await invoke("repair_fetch_persons", { workId });
}

export async function refreshTmdbMetadata(workId: number): Promise<void> {
  await invoke("refresh_tmdb_metadata", { workId });
}
