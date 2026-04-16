import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { listWorks, getWork, getFilterOptions } from "@/api/works";
import type { ListWorksParams } from "@/api/works";
import { updateUserStats, recordPlay } from "@/api/stats";
import { listWorkTags } from "@/api/tags";
import { useLibraryStore } from "@/store/libraryStore";
import type { SortField, SortOrder } from "@cinemantis/shared-types";

// ── Query key factory ────────────────────────────────────────────────────────

export const workKeys = {
  all: ["works"] as const,
  list: (params: object) => ["works", "list", params] as const,
  detail: (id: number) => ["works", "detail", id] as const,
  tags: (id: number) => ["works", "tags", id] as const,
  filterOptions: ["works", "filterOptions"] as const,
};

// ── Smart collection derivation ──────────────────────────────────────────────

type SmartOverride = {
  workType?: string | null;
  watchStatus?: string | null;
  isFavorite?: boolean;
  minUserRating?: number;
  minPlayCount?: number;
  matchStatus?: string | null;
  sortField?: SortField;
  sortOrder?: SortOrder;
};

function getSmartOverride(section: string): SmartOverride {
  switch (section) {
    case "all-movies":      return { workType: "movie" };
    case "all-drama":       return { workType: "drama" };
    case "unwatched":       return { watchStatus: "unwatched" };
    case "watching":        return { watchStatus: "watching" };
    case "recently-added":    return { sortField: "created_at",    sortOrder: "desc" };
    case "recently-played":   return { sortField: "last_played_at", sortOrder: "desc", minPlayCount: 1 };
    case "high-rated":        return { minUserRating: 4, sortField: "user_rating", sortOrder: "desc" };
    case "favorites":         return { isFavorite: true, sortField: "title", sortOrder: "asc" };
    case "continue-watching": return { watchStatus: "watching",  sortField: "last_played_at", sortOrder: "desc" };
    case "needs-attention":   return { matchStatus: "unmatched", sortField: "created_at",     sortOrder: "desc" };
    case "completed":         return { watchStatus: "watched",   sortField: "last_played_at", sortOrder: "desc" };
    case "stalled":           return { watchStatus: "watching",  sortField: "last_played_at", sortOrder: "asc" };
    default:                  return {};
  }
}

// ── Work list ────────────────────────────────────────────────────────────────

export function useWorkList() {
  const { activeSection, filters, sortField, sortOrder } = useLibraryStore();

  const smart = getSmartOverride(activeSection);

  const params: ListWorksParams = {
    // smart overrides → then user filters
    workType:       smart.workType      !== undefined ? smart.workType      : (filters.workType ?? null),
    watchStatus:    smart.watchStatus   !== undefined ? smart.watchStatus   : (filters.watchStatus ?? null),
    isFavorite:     smart.isFavorite    !== undefined ? smart.isFavorite    : (filters.isFavorite || null),
    minUserRating:  smart.minUserRating !== undefined ? smart.minUserRating : (filters.minUserRating ?? null),
    minPlayCount:   smart.minPlayCount  !== undefined ? smart.minPlayCount  : null,
    sortField:      smart.sortField     !== undefined ? smart.sortField     : sortField,
    sortOrder:      smart.sortOrder     !== undefined ? smart.sortOrder     : sortOrder,
    // passthrough filters
    matchStatus:    filters.matchStatus,
    query:          filters.query       || null,
    yearFrom:       filters.yearFrom,
    yearTo:         filters.yearTo,
    genre:          filters.genre,
    country:        filters.country,
    tagIds:         filters.tagIds,
  };

  return useQuery({
    queryKey: workKeys.list(params),
    queryFn: () => listWorks(params),
  });
}

// ── Filter options (genres, countries, year range) ────────────────────────────

export function useFilterOptions() {
  return useQuery({
    queryKey: workKeys.filterOptions,
    queryFn: getFilterOptions,
    staleTime: 5 * 60 * 1000, // 5分キャッシュ
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
