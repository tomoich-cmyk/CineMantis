import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import {
  listSeries,
  getSeriesDetail,
  getSeriesWorks,
  createSeries,
  addToSeries,
  removeFromSeries,
  deleteSeries,
} from "@/api/series";

// ─── Query key factory ───────────────────────────────────────────────────────

export const seriesKeys = {
  all: ["series"] as const,
  list: () => ["series", "list"] as const,
  detail: (id: number) => ["series", "detail", id] as const,
  works: (id: number) => ["series", "works", id] as const,
};

// ─── Queries ─────────────────────────────────────────────────────────────────

export function useSeriesList() {
  return useQuery({
    queryKey: seriesKeys.list(),
    queryFn: listSeries,
    staleTime: 1000 * 30,
  });
}

export function useSeriesDetail(seriesId: number | null) {
  return useQuery({
    queryKey: seriesKeys.detail(seriesId ?? -1),
    queryFn: () => getSeriesDetail(seriesId!),
    enabled: seriesId !== null,
  });
}

export function useSeriesWorks(seriesId: number | null) {
  return useQuery({
    queryKey: seriesKeys.works(seriesId ?? -1),
    queryFn: () => getSeriesWorks(seriesId!),
    enabled: seriesId !== null,
  });
}

// ─── Mutations ───────────────────────────────────────────────────────────────

export function useCreateSeries() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ title, seriesType }: { title: string; seriesType?: string }) =>
      createSeries(title, seriesType),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: seriesKeys.list() });
    },
  });
}

export function useDeleteSeries() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: deleteSeries,
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: seriesKeys.list() });
    },
  });
}

export function useAddToSeries() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({
      seriesId,
      workId,
      sortOrder,
    }: {
      seriesId: number;
      workId: number;
      sortOrder?: number;
    }) => addToSeries(seriesId, workId, sortOrder),
    onSuccess: (_data, vars) => {
      qc.invalidateQueries({ queryKey: seriesKeys.works(vars.seriesId) });
      qc.invalidateQueries({ queryKey: seriesKeys.list() });
    },
  });
}

export function useRemoveFromSeries() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ seriesId, workId }: { seriesId: number; workId: number }) =>
      removeFromSeries(seriesId, workId),
    onSuccess: (_data, vars) => {
      qc.invalidateQueries({ queryKey: seriesKeys.works(vars.seriesId) });
      qc.invalidateQueries({ queryKey: seriesKeys.list() });
    },
  });
}
