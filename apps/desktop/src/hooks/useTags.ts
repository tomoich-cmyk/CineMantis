import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { listTags, addTag, tagWork, untagWork, listWorkTags } from "@/api/tags";

export const tagKeys = {
  all: ["tags"] as const,
  list: () => ["tags", "list"] as const,
  workTags: (workId: number) => ["tags", "work", workId] as const,
};

export function useTags() {
  return useQuery({
    queryKey: tagKeys.list(),
    queryFn: listTags,
    staleTime: 5 * 60 * 1000,
  });
}

export function useWorkTagsQuery(workId: number | null) {
  return useQuery({
    queryKey: tagKeys.workTags(workId ?? -1),
    queryFn: () => listWorkTags(workId!),
    enabled: workId !== null,
  });
}

export function useAddTag() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ name, color }: { name: string; color?: string }) =>
      addTag(name, color),
    onSuccess: () => qc.invalidateQueries({ queryKey: tagKeys.list() }),
  });
}

export function useTagWork() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ workId, tagId }: { workId: number; tagId: number }) =>
      tagWork(workId, tagId),
    onSuccess: (_data, { workId }) => {
      qc.invalidateQueries({ queryKey: tagKeys.workTags(workId) });
    },
  });
}

export function useUntagWork() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: ({ workId, tagId }: { workId: number; tagId: number }) =>
      untagWork(workId, tagId),
    onSuccess: (_data, { workId }) => {
      qc.invalidateQueries({ queryKey: tagKeys.workTags(workId) });
    },
  });
}
