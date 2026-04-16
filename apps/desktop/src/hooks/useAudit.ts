import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import {
  getDuplicateGroups,
  getIntegrityReport,
  deleteWork,
  repairFetchPersons,
  refreshTmdbMetadata,
} from "@/api/audit";

export const auditKeys = {
  duplicates: ["audit", "duplicates"] as const,
  integrity: ["audit", "integrity"] as const,
};

export function useDuplicateGroups() {
  return useQuery({
    queryKey: auditKeys.duplicates,
    queryFn: getDuplicateGroups,
    staleTime: 30_000,
  });
}

export function useIntegrityReport() {
  return useQuery({
    queryKey: auditKeys.integrity,
    queryFn: getIntegrityReport,
    staleTime: 30_000,
  });
}

export function useDeleteWork() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (workId: number) => deleteWork(workId),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["works"] });
      qc.invalidateQueries({ queryKey: auditKeys.duplicates });
      qc.invalidateQueries({ queryKey: auditKeys.integrity });
    },
  });
}

export function useRepairFetchPersons() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (workId: number) => repairFetchPersons(workId),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["works"] });
      qc.invalidateQueries({ queryKey: auditKeys.integrity });
    },
  });
}

export function useRefreshTmdbMetadata() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (workId: number) => refreshTmdbMetadata(workId),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["works"] });
      qc.invalidateQueries({ queryKey: auditKeys.integrity });
    },
  });
}
