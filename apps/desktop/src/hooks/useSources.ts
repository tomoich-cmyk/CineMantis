import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { listSources, addSource, scanSource } from "@/api/sources";

export const sourceKeys = {
  all: ["sources"] as const,
  list: () => ["sources", "list"] as const,
};

export function useSourceList() {
  return useQuery({
    queryKey: sourceKeys.list(),
    queryFn: listSources,
  });
}

export function useAddSource() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: addSource,
    onSuccess: () => qc.invalidateQueries({ queryKey: sourceKeys.all }),
  });
}

export function useScanSource() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: scanSource,
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: sourceKeys.all });
      qc.invalidateQueries({ queryKey: ["works"] });
    },
  });
}
