import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { listWorks, getWork } from "@/api/works";
import { updateUserStats, recordPlay } from "@/api/stats";
import { listWorkTags } from "@/api/tags";
import { useLibraryStore } from "@/store/libraryStore";

// ── Query key factory ────────────────────────────────────────────────────────

export const workKeys = {
  all: ["works"] as const,
  list: (params: object) => ["works", "list", params] as const,
  detail: (id: number) => ["works", "detail", id] as const,
  tags: (id: number) => ["works", "tags", id] as const,
};

// ── Work list ────────────────────────────────────────────────────────────────

export function useWorkList() {
  const { activeSection, filters, sortField, sortOrder } = useLibraryStore();

  // Derive workType / watchStatus from nav section
  const workType =
    activeSection === "all-movies" ? "movie"
    : activeSection === "all-drama" ? "drama"
    : null;

  const watchStatus =
    activeSection === "unwatched" ? "unwatched"
    : activeSection === "watching" ? "watching"
    : filters.watchStatus;

  const params = {
    workType: workType ?? filters.workType,
    watchStatus,
    query: filters.query || null,
    sortField,
    sortOrder,
  };

  return useQuery({
    queryKey: workKeys.list(params),
    queryFn: () => listWorks(params),
  });
}

// ── Work detail ──────────────────────────────────────────────────────────────

export function useWorkDetail(workId: number | null) {
  return useQuery({
    queryKey: workKeys.detail(workId ?? -1),
    queryFn: () => getWork(workId!),
    enabled: workId !== null,
  });
}

// ── Work tags ────────────────────────────────────────────────────────────────

export function useWorkTags(workId: number | null) {
  return useQuery({
    queryKey: workKeys.tags(workId ?? -1),
    queryFn: () => listWorkTags(workId!),
    enabled: workId !== null,
  });
}

// ── Mutations ────────────────────────────────────────────────────────────────

export function useUpdateStats() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: updateUserStats,
    onSuccess: (_data, vars) => {
      qc.invalidateQueries({ queryKey: workKeys.detail(vars.work_id) });
      qc.invalidateQueries({ queryKey: workKeys.all });
    },
  });
}

export function useRecordPlay() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: recordPlay,
    onSuccess: (_data, workId) => {
      qc.invalidateQueries({ queryKey: workKeys.detail(workId) });
      qc.invalidateQueries({ queryKey: workKeys.all });
    },
  });
}
