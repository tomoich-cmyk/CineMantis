import { useEffect } from "react";
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { listen } from "@tauri-apps/api/event";
import {
  searchTmdbCandidates,
  autoMatchWork,
  applyTmdbMatch,
  refreshTmdbMetadata,
  clearTmdbMatch,
  unlockTmdbMatch,
  type MetadataUpdatedEvent,
} from "@/api/tmdb";
import { workKeys } from "./useWorks";
import { auditKeys } from "./useAudit";

// ─── Query key factory ───────────────────────────────────────────────────────

export const tmdbKeys = {
  candidates: (workId: number) => ["tmdb", "candidates", workId] as const,
};

// ─── Candidates ──────────────────────────────────────────────────────────────

export interface CandidateSearchParams {
  queryOverride?: string | null;
  mediaTypeHint?: "movie" | "tv" | null;
}

export function useTmdbCandidates(
  workId: number | null,
  params?: CandidateSearchParams,
) {
  return useQuery({
    queryKey: [...tmdbKeys.candidates(workId ?? -1), params?.queryOverride ?? "", params?.mediaTypeHint ?? ""],
    queryFn: () =>
      searchTmdbCandidates(workId!, params?.queryOverride, params?.mediaTypeHint),
    enabled: workId !== null,
    staleTime: 1000 * 60 * 10, // 10 分キャッシュ
  });
}

// ─── Mutations ───────────────────────────────────────────────────────────────

export function useAutoMatchWork() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: autoMatchWork,
    onSuccess: (_data, workId) => {
      qc.invalidateQueries({ queryKey: workKeys.detail(workId) });
      qc.invalidateQueries({ queryKey: workKeys.all });
    },
  });
}

export function useApplyTmdbMatch() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({
      workId,
      tmdbId,
      mediaType,
      lock,
    }: {
      workId: number;
      tmdbId: number;
      mediaType: string;
      lock: boolean;
    }) => applyTmdbMatch(workId, tmdbId, mediaType, lock),
    onSuccess: (_data, vars) => {
      qc.invalidateQueries({ queryKey: workKeys.detail(vars.workId) });
      qc.invalidateQueries({ queryKey: workKeys.all });
      qc.invalidateQueries({ queryKey: tmdbKeys.candidates(vars.workId) });
      qc.invalidateQueries({ queryKey: auditKeys.integrity });
    },
  });
}

export function useRefreshTmdbMetadata() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: refreshTmdbMetadata,
    onSuccess: (_data, workId) => {
      qc.invalidateQueries({ queryKey: workKeys.detail(workId) });
      qc.invalidateQueries({ queryKey: workKeys.all });
    },
  });
}

export function useClearTmdbMatch() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: clearTmdbMatch,
    onSuccess: (_data, workId) => {
      qc.invalidateQueries({ queryKey: workKeys.detail(workId) });
      qc.invalidateQueries({ queryKey: workKeys.all });
    },
  });
}

export function useUnlockTmdbMatch() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: unlockTmdbMatch,
    onSuccess: (_data, workId) => {
      qc.invalidateQueries({ queryKey: workKeys.detail(workId) });
      qc.invalidateQueries({ queryKey: workKeys.all });
    },
  });
}

// ─── Global metadata event listener ─────────────────────────────────────────

/**
 * `metadata:updated` イベントをグローバル購読。
 * App.tsx トップレベルで一度だけマウントする。
 */
export function useMetadataEvents() {
  const qc = useQueryClient();

  useEffect(() => {
    let unlisten: (() => void) | undefined;

    listen<MetadataUpdatedEvent>("metadata:updated", (event) => {
      const { work_id } = event.payload;
      qc.invalidateQueries({ queryKey: workKeys.detail(work_id) });
      qc.invalidateQueries({ queryKey: workKeys.all });
    }).then((fn) => {
      unlisten = fn;
    });

    return () => unlisten?.();
  }, [qc]);
}
