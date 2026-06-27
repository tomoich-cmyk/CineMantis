import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { listSyncOutbox, processSyncOutbox } from "@/api/sync";
import { sourceKeys } from "./useSources";

export const syncKeys = {
  all: ["syncOutbox"] as const,
  list: () => ["syncOutbox", "list"] as const,
};

export function useSyncOutboxList() {
  return useQuery({
    queryKey: syncKeys.list(),
    queryFn: listSyncOutbox,
  });
}

export function useProcessSyncOutbox() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: processSyncOutbox,
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: syncKeys.all });
      qc.invalidateQueries({ queryKey: sourceKeys.all });
      qc.invalidateQueries({ queryKey: ["works"] });
    },
  });
}
