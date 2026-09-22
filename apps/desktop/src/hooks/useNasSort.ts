import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { planNasSort, executeNasSort } from "@/api/nasSort";
import { workKeys } from "@/hooks/useWorks";

export const nasSortKeys = {
  plan: (sourceId: number | null) => ["nasSort", "plan", sourceId] as const,
};

export function useNasSortPlan(sourceId: number | null) {
  return useQuery({
    queryKey: nasSortKeys.plan(sourceId),
    queryFn: () => planNasSort(sourceId!),
    enabled: sourceId !== null,
  });
}

export function useExecuteNasSort(sourceId: number | null) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (workIds: number[]) => executeNasSort(sourceId!, workIds),
    onSuccess: () => {
      // ファイルの所属ソースとパスが変わるので一覧も作り直す
      qc.invalidateQueries({ queryKey: nasSortKeys.plan(sourceId) });
      qc.invalidateQueries({ queryKey: workKeys.all });
    },
  });
}
