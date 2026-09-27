import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  deferGtReviewTask,
  getGtReviewTask,
  listGtReviewTasks,
  previewGtReviewTmdbTarget,
  resolveGtReviewConfirm,
  resolveGtReviewNone,
  resolveGtReviewPickOther,
  resumeGtReviewTask,
  type ReviewState,
} from "@/api/gtReview";

export const gtReviewKeys = {
  all: ["gtReview"] as const,
  list: (sampleId: string, states?: ReviewState[]) =>
    ["gtReview", "list", sampleId, states ?? "all"] as const,
  task: (taskId: number) => ["gtReview", "task", taskId] as const,
};

/**
 * sample の未解決課題を読む。
 *
 * backend は記録が壊れていればエラーを返す（黙って件数を減らさない）。
 * ここでも握りつぶさず、そのまま UI へ出す。`retry: false` なのは、
 * fail-closed のエラーを何度投げ直しても結果が変わらないため。
 */
export function useGtReviewTasks(sampleId: string, states?: ReviewState[], enabled = true) {
  return useQuery({
    queryKey: gtReviewKeys.list(sampleId, states),
    queryFn: () => listGtReviewTasks(sampleId, states),
    enabled: enabled && sampleId.trim().length > 0,
    retry: false,
    staleTime: 10_000,
  });
}

export function useGtReviewTask(taskId: number | null) {
  return useQuery({
    queryKey: gtReviewKeys.task(taskId ?? -1),
    queryFn: () => getGtReviewTask(taskId as number),
    enabled: taskId !== null,
    retry: false,
  });
}

/** ready ⇄ deferred */
export function useGtReviewStateChange() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ taskId, next }: { taskId: number; next: "defer" | "resume" }) =>
      next === "defer" ? deferGtReviewTask(taskId) : resumeGtReviewTask(taskId),
    retry: false,
    onSuccess: (_result, { taskId }) => {
      qc.invalidateQueries({ queryKey: gtReviewKeys.all });
      qc.invalidateQueries({ queryKey: gtReviewKeys.task(taskId) });
    },
  });
}

/**
 * 候補外 TMDB ID のプレビュー。
 *
 * mutation にしているのは、入力するたびに勝手に通信させないため。
 * `retry: false` なので、見つからない ID を何度も叩きにいかない。
 * ここで確かめても、確定時に backend がもう一度確認する。
 */
export function useGtReviewTmdbPreview() {
  return useMutation({
    mutationFn: ({ tmdbId, mediaType }: { tmdbId: number; mediaType: string }) =>
      previewGtReviewTmdbTarget(tmdbId, mediaType),
    retry: false,
  });
}

type ResolveInput =
  | { kind: "confirm"; taskId: number; candidateId: number; note?: string }
  | { kind: "pickOther"; taskId: number; tmdbId: number; mediaType: string; note?: string }
  | { kind: "none"; taskId: number; note?: string };

/** confirm / pick_other / none をひとつの mutation にまとめる */
export function useGtReviewResolve() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (input: ResolveInput) => {
      switch (input.kind) {
        case "confirm":
          return resolveGtReviewConfirm(input.taskId, input.candidateId, input.note);
        case "pickOther":
          return resolveGtReviewPickOther(
            input.taskId,
            input.tmdbId,
            input.mediaType,
            input.note,
          );
        case "none":
          return resolveGtReviewNone(input.taskId, input.note);
      }
    },
    retry: false,
    onSuccess: (_result, input) => {
      // 解決済みの課題は一覧から消える。詳細は残しておく意味が無いので捨てる
      qc.invalidateQueries({ queryKey: gtReviewKeys.all });
      qc.removeQueries({ queryKey: gtReviewKeys.task(input.taskId) });
    },
  });
}
