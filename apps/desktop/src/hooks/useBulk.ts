import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import {
  getAttentionStats,
  bulkSetWatchStatus,
  bulkSetFavorite,
  bulkAddTag,
  bulkRemoveTag,
  bulkSetMatchStatus,
} from "@/api/bulk";
import { workKeys } from "@/hooks/useWorks";
import { useLibraryStore } from "@/store/libraryStore";

// ── Attention stats ─────────────────────────────────────────────────────────

export const bulkKeys = {
  attentionStats: ["bulk", "attentionStats"] as const,
};

export function useAttentionStats() {
  return useQuery({
    queryKey: bulkKeys.attentionStats,
    queryFn: getAttentionStats,
    staleTime: 60 * 1000, // 1分キャッシュ
  });
}

// ── Bulk mutations ─────────────────────────────────────────────────────────

function useInvalidateAll() {
  const qc = useQueryClient();
  return () => {
    qc.invalidateQueries({ queryKey: workKeys.all });
    qc.invalidateQueries({ queryKey: bulkKeys.attentionStats });
  };
}

export function useBulkSetWatchStatus() {
  const { clearSelection } = useLibraryStore();
  const invalidate = useInvalidateAll();
  return useMutation({
    mutationFn: ({
      workIds,
      status,
    }: {
      workIds: number[];
      status: "unwatched" | "watching" | "watched" | "skipped";
    }) => bulkSetWatchStatus(workIds, status),
    onSuccess: () => {
      invalidate();
      clearSelection();
    },
  });
}

export function useBulkSetFavorite() {
  const { clearSelection } = useLibraryStore();
  const invalidate = useInvalidateAll();
  return useMutation({
    mutationFn: ({
      workIds,
      isFavorite,
    }: {
      workIds: number[];
      isFavorite: boolean;
    }) => bulkSetFavorite(workIds, isFavorite),
    onSuccess: () => {
      invalidate();
      clearSelection();
    },
  });
}

export function useBulkAddTag() {
  const { clearSelection } = useLibraryStore();
  const invalidate = useInvalidateAll();
  return useMutation({
    mutationFn: ({ workIds, tagId }: { workIds: number[]; tagId: number }) =>
      bulkAddTag(workIds, tagId),
    onSuccess: () => {
      invalidate();
      clearSelection();
    },
  });
}

export function useBulkRemoveTag() {
  const { clearSelection } = useLibraryStore();
  const invalidate = useInvalidateAll();
  return useMutation({
    mutationFn: ({ workIds, tagId }: { workIds: number[]; tagId: number }) =>
      bulkRemoveTag(workIds, tagId),
    onSuccess: () => {
      invalidate();
      clearSelection();
    },
  });
}

export function useBulkSetMatchStatus() {
  const { clearSelection } = useLibraryStore();
  const invalidate = useInvalidateAll();
  return useMutation({
    mutationFn: ({
      workIds,
      status,
    }: {
      workIds: number[];
      status: "locked" | "manual" | "unmatched";
    }) => bulkSetMatchStatus(workIds, status),
    onSuccess: () => {
      invalidate();
      clearSelection();
    },
  });
}
