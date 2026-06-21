import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  addWorkAwardResult,
  deleteWorkAwardResult,
  getAwardBodyDetail,
  listAwardBodies,
  listAwardCategories,
  listAwardWinningWorks,
  listWorkAwards,
  updateWorkAwardResult,
} from "@/api/awards";
import type { AddWorkAwardResultInput, AwardWorkFilter, UpdateWorkAwardResultInput } from "@/api/awards";

export const awardKeys = {
  all: ["awards"] as const,
  bodies: ["awards", "bodies"] as const,
  body: (id: number) => ["awards", "body", id] as const,
  categories: (bodyId: number) => ["awards", "categories", bodyId] as const,
  work: (workId: number) => ["awards", "work", workId] as const,
  works: (filter: AwardWorkFilter) => ["awards", "works", filter] as const,
};

export function useAwardBodies() {
  return useQuery({
    queryKey: awardKeys.bodies,
    queryFn: listAwardBodies,
  });
}

export function useAwardBodyDetail(awardBodyId: number | null) {
  return useQuery({
    queryKey: awardKeys.body(awardBodyId ?? -1),
    queryFn: () => getAwardBodyDetail(awardBodyId!),
    enabled: awardBodyId !== null,
  });
}

export function useAwardCategories(awardBodyId?: number | null) {
  return useQuery({
    queryKey: awardKeys.categories(awardBodyId ?? -1),
    queryFn: () => listAwardCategories(awardBodyId!),
    enabled: !!awardBodyId,
  });
}

export function useWorkAwards(workId?: number | null) {
  return useQuery({
    queryKey: awardKeys.work(workId ?? -1),
    queryFn: () => listWorkAwards(workId!),
    enabled: !!workId,
  });
}

export function useAwardWinningWorks(filter: AwardWorkFilter) {
  return useQuery({
    queryKey: awardKeys.works(filter),
    queryFn: () => listAwardWinningWorks(filter),
  });
}

export function useAddWorkAwardResult(workId: number) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (input: AddWorkAwardResultInput) => addWorkAwardResult(input),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: awardKeys.all });
      qc.invalidateQueries({ queryKey: ["works"] });
      qc.invalidateQueries({ queryKey: awardKeys.work(workId) });
    },
  });
}

export function useUpdateWorkAwardResult(workId: number) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (input: UpdateWorkAwardResultInput) => updateWorkAwardResult(input),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: awardKeys.all });
      qc.invalidateQueries({ queryKey: awardKeys.work(workId) });
    },
  });
}

export function useDeleteWorkAwardResult(workId: number) {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: deleteWorkAwardResult,
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: awardKeys.all });
      qc.invalidateQueries({ queryKey: ["works"] });
      qc.invalidateQueries({ queryKey: awardKeys.work(workId) });
    },
  });
}
