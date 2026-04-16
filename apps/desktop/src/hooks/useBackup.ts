import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import {
  backupDatabase,
  listBackups,
  restoreDatabase,
  deleteBackup,
} from "@/api/backup";

const backupKeys = {
  list: ["backups", "list"] as const,
};

export function useBackupList() {
  return useQuery({
    queryKey: backupKeys.list,
    queryFn: listBackups,
  });
}

export function useCreateBackup() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (label?: string) => backupDatabase(label),
    onSuccess: () => qc.invalidateQueries({ queryKey: backupKeys.list }),
  });
}

export function useRestoreDatabase() {
  return useMutation({
    mutationFn: (backupPath: string) => restoreDatabase(backupPath),
    // 成功後は「再起動してください」と表示するだけ (onSuccess は呼び出し側で処理)
  });
}

export function useDeleteBackup() {
  const qc = useQueryClient();
  return useMutation({
    mutationFn: (backupPath: string) => deleteBackup(backupPath),
    onSuccess: () => qc.invalidateQueries({ queryKey: backupKeys.list }),
  });
}
