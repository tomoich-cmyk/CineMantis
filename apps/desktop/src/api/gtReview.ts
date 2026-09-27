import { invoke } from "@tauri-apps/api/core";

// ─── 型定義 ───────────────────────────────────────────────────────────────────
//
// ground truth レビュー（PR3 C5c.3B）。
//
// backend が返すのは **抽出時に凍結された材料だけ**で、いまの works の値も、
// rules / Jev の score・順位・判定も入っていない。UI 側でもそれを足さない
// （機械の答えを見てから人が決めると、その GT で測った精度が歪む）。

export type ReviewState = "ready" | "deferred" | "resolved";

export interface ReviewTaskSummary {
  taskId: number;
  workId: number;
  runId: number;
  sampleId: string;
  sampleRank: number;
  state: ReviewState;
  /** run が照合に使った題名（works.title ではない） */
  displayTitle: string;
  /** run の時点でのファイル数 */
  partCount: number;
  candidateCount: number;
  createdAt: string;
}

export interface ReviewCandidate {
  candidateId: number;
  tmdbId: number;
  mediaType: string;
  /** run の時点で TMDB から見えていた内容 */
  tmdbSnapshot: Record<string, unknown>;
}

export interface ReviewTaskDetail {
  taskId: number;
  workId: number;
  runId: number;
  sampleId: string;
  sampleRank: number;
  state: ReviewState;
  cohort: string;
  displayTitle: string;
  partCount: number;
  /** run が見ていた手元の証拠（凍結済み） */
  evidence: Record<string, unknown>;
  candidates: ReviewCandidate[];
}

export interface VerifiedTarget {
  tmdbId: number;
  mediaType: string;
  title: string;
  originalTitle: string | null;
  year: string | null;
  overview: string | null;
  posterPath: string | null;
}

export interface GtReviewResolution {
  taskId: number;
  labelId: number;
  state: string;
}

// ─── 値の検証 ─────────────────────────────────────────────────────────────────

/**
 * backend の state をそのまま信じない。
 *
 * `as ReviewState` で通すと、`stale` のような UI が扱えない状態や、将来増えた
 * 未知の状態が「型の上では正しい値」として画面に流れ込む。ground truth の画面は
 * 分からないものを黙って通さない方針なので、ここで投げる。
 */
function toReviewState(value: string): ReviewState {
  if (value === "ready" || value === "deferred" || value === "resolved") {
    return value;
  }
  throw new Error(`GT レビューの状態が不正です: ${value}`);
}

// ─── camelCase 変換 ───────────────────────────────────────────────────────────
//
// backend の失敗は握りつぶさない。fail-closed なエラー（記録の破損や
// run の不整合）を空配列に化かすと、母数が静かに縮んで GT が壊れる。
// invoke が reject したらそのまま投げる。

function toSummary(raw: {
  task_id: number;
  work_id: number;
  run_id: number;
  sample_id: string;
  sample_rank: number;
  state: string;
  display_title: string;
  part_count: number;
  candidate_count: number;
  created_at: string;
}): ReviewTaskSummary {
  return {
    taskId: raw.task_id,
    workId: raw.work_id,
    runId: raw.run_id,
    sampleId: raw.sample_id,
    sampleRank: raw.sample_rank,
    state: toReviewState(raw.state),
    displayTitle: raw.display_title,
    partCount: raw.part_count,
    candidateCount: raw.candidate_count,
    createdAt: raw.created_at,
  };
}

function toCandidate(raw: {
  candidate_id: number;
  tmdb_id: number;
  media_type: string;
  tmdb_snapshot: Record<string, unknown>;
}): ReviewCandidate {
  return {
    candidateId: raw.candidate_id,
    tmdbId: raw.tmdb_id,
    mediaType: raw.media_type,
    tmdbSnapshot: raw.tmdb_snapshot,
  };
}

function toDetail(raw: {
  task_id: number;
  work_id: number;
  run_id: number;
  sample_id: string;
  sample_rank: number;
  state: string;
  cohort: string;
  display_title: string;
  part_count: number;
  evidence: Record<string, unknown>;
  candidates: Parameters<typeof toCandidate>[0][];
}): ReviewTaskDetail {
  return {
    taskId: raw.task_id,
    workId: raw.work_id,
    runId: raw.run_id,
    sampleId: raw.sample_id,
    sampleRank: raw.sample_rank,
    state: toReviewState(raw.state),
    cohort: raw.cohort,
    displayTitle: raw.display_title,
    partCount: raw.part_count,
    evidence: raw.evidence,
    // backend が決めた並びをそのまま保つ（UI で並べ替えない）
    candidates: raw.candidates.map(toCandidate),
  };
}

function toVerifiedTarget(raw: {
  tmdb_id: number;
  media_type: string;
  title: string;
  original_title: string | null;
  year: string | null;
  overview: string | null;
  poster_path: string | null;
}): VerifiedTarget {
  return {
    tmdbId: raw.tmdb_id,
    mediaType: raw.media_type,
    title: raw.title,
    originalTitle: raw.original_title,
    year: raw.year,
    overview: raw.overview,
    posterPath: raw.poster_path,
  };
}

function toResolution(raw: {
  task_id: number;
  label_id: number;
  state: string;
}): GtReviewResolution {
  return { taskId: raw.task_id, labelId: raw.label_id, state: raw.state };
}

// ─── コマンド ─────────────────────────────────────────────────────────────────

export async function listGtReviewTasks(
  sampleId: string,
  states?: ReviewState[],
): Promise<ReviewTaskSummary[]> {
  const rows = await invoke<Parameters<typeof toSummary>[0][]>("list_gt_review_tasks", {
    sampleId,
    states: states ?? null,
  });
  return rows.map(toSummary);
}

export async function getGtReviewTask(taskId: number): Promise<ReviewTaskDetail> {
  const raw = await invoke<Parameters<typeof toDetail>[0]>("get_gt_review_task", { taskId });
  return toDetail(raw);
}

/** 「後で」にする。動かせなかったら false（他所で状態が変わっている） */
export function deferGtReviewTask(taskId: number): Promise<boolean> {
  return invoke<boolean>("defer_gt_review_task", { taskId });
}

/** 「後で」から戻す */
export function resumeGtReviewTask(taskId: number): Promise<boolean> {
  return invoke<boolean>("resume_gt_review_task", { taskId });
}

/** 候補外 TMDB ID を確かめるだけ（DB は変わらない） */
export async function previewGtReviewTmdbTarget(
  tmdbId: number,
  mediaType: string,
): Promise<VerifiedTarget> {
  const raw = await invoke<Parameters<typeof toVerifiedTarget>[0]>(
    "preview_gt_review_tmdb_target",
    { tmdbId, mediaType },
  );
  return toVerifiedTarget(raw);
}

export async function resolveGtReviewConfirm(
  taskId: number,
  candidateId: number,
  note?: string,
): Promise<GtReviewResolution> {
  const raw = await invoke<Parameters<typeof toResolution>[0]>("resolve_gt_review_confirm", {
    taskId,
    candidateId,
    note: note ?? null,
  });
  return toResolution(raw);
}

export async function resolveGtReviewPickOther(
  taskId: number,
  tmdbId: number,
  mediaType: string,
  note?: string,
): Promise<GtReviewResolution> {
  const raw = await invoke<Parameters<typeof toResolution>[0]>("resolve_gt_review_pick_other", {
    taskId,
    tmdbId,
    mediaType,
    note: note ?? null,
  });
  return toResolution(raw);
}

export async function resolveGtReviewNone(
  taskId: number,
  note?: string,
): Promise<GtReviewResolution> {
  const raw = await invoke<Parameters<typeof toResolution>[0]>("resolve_gt_review_none", {
    taskId,
    note: note ?? null,
  });
  return toResolution(raw);
}
