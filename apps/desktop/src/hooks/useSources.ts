import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { listSources, addSource, deleteSource, deduplicateLibraryFiles, scanSource, type ScanResult } from "@/api/sources";

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
  return useMutation<ScanResult, Error, number>({
    mutationFn: scanSource,
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: sourceKeys.all });
      qc.invalidateQueries({ queryKey: ["works"] });
      qc.invalidateQueries({ queryKey: ["syncOutbox"] });
    },
    onError: (err: unknown) => {
      console.error("Scan failed:", err);
    },
  });
}

export function useDeleteSource() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: deleteSource,
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: sourceKeys.all });
      qc.invalidateQueries({ queryKey: ["works"] });
      qc.invalidateQueries({ queryKey: ["syncOutbox"] });
    },
  });
}

export function useDeduplicateLibraryFiles() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: deduplicateLibraryFiles,
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: sourceKeys.all });
      qc.invalidateQueries({ queryKey: ["works"] });
      qc.invalidateQueries({ queryKey: ["syncOutbox"] });
    },
  });
}
