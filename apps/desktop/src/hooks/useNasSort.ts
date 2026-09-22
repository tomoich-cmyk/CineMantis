import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { planNasSort, executeNasSort } from "@/api/nasSort";
import { workKeys } from "@/hooks/useWorks";

export const nasSortKeys = {
  plan: (folder: string | null) => ["nasSort", "plan", folder] as const,
};

export function useNasSortPlan(folder: string | null) {
  return useQuery({
    queryKey: nasSortKeys.plan(folder),
    queryFn: () => planNasSort(folder!),
    enabled: folder !== null,
  });
}

export function useExecuteNasSort(folder: string | null) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (workIds: number[]) => executeNasSort(folder!, workIds),
    onSuccess: () => {
      // ファイルの所属ソースとパスが変わるので一覧も作り直す
      qc.invalidateQueries({ queryKey: nasSortKeys.plan(folder) });
      qc.invalidateQueries({ queryKey: workKeys.all });
    },
  });
}
