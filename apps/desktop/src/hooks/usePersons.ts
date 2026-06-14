import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { listPersons, getPerson, getWorkPersons, getPersonWorks, localizePersonNames, syncMissingPersons } from "@/api/persons";

// ─── Query key factory ───────────────────────────────────────────────────────

export const personKeys = {
  all: ["persons"] as const,
  list: (role?: string | null) => ["persons", "list", role ?? "all"] as const,
  detail: (id: number) => ["persons", "detail", id] as const,
  workPersons: (workId: number) => ["persons", "work", workId] as const,
  personWorks: (id: number, role?: string | null) => ["persons", "works", id, role ?? "all"] as const,
};

// ─── Queries ─────────────────────────────────────────────────────────────────

export function usePersonsList(roleFilter?: string | null) {
  return useQuery({
    queryKey: personKeys.list(roleFilter),
    queryFn: () => listPersons(roleFilter),
    staleTime: 1000 * 60,
  });
}

export function usePersonDetail(personId: number | null) {
  return useQuery({
    queryKey: personKeys.detail(personId ?? -1),
    queryFn: () => getPerson(personId!),
    enabled: personId !== null,
  });
}

/** DetailPane の人物リスト取得 */
export function useWorkPersons(workId: number | null) {
  return useQuery({
    queryKey: personKeys.workPersons(workId ?? -1),
    queryFn: () => getWorkPersons(workId!),
    enabled: workId !== null,
  });
}

/** 人物詳細画面の作品一覧 */
export function usePersonWorks(personId: number | null, roleFilter?: string | null) {
  return useQuery({
    queryKey: personKeys.personWorks(personId ?? -1, roleFilter),
    queryFn: () => getPersonWorks(personId!, roleFilter),
    enabled: personId !== null,
  });
}

export function useSyncMissingPersons() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: syncMissingPersons,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: personKeys.all });
      queryClient.invalidateQueries({ queryKey: ["works"] });
    },
  });
}

export function useLocalizePersonNames() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: localizePersonNames,
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: personKeys.all });
      queryClient.invalidateQueries({ queryKey: ["works"] });
    },
  });
}
