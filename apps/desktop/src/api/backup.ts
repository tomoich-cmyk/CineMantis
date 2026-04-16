import { invoke } from "@tauri-apps/api/core";

export interface BackupEntry {
  name: string;
  path: string;
  size_bytes: number;
  created_at: string;
}

/** DB を backups/ ディレクトリへ VACUUM INTO コピーする */
export async function backupDatabase(label?: string): Promise<string> {
  return invoke<string>("backup_database", { label: label ?? null });
}

/** バックアップ一覧を新しい順に返す */
export async function listBackups(): Promise<BackupEntry[]> {
  return invoke<BackupEntry[]>("list_backups");
}

/** 指定したバックアップを pending_restore としてマーク（再起動後に適用） */
export async function restoreDatabase(backupPath: string): Promise<void> {
  return invoke("restore_database", { backupPath });
}

/** バックアップファイルを削除 */
export async function deleteBackup(backupPath: string): Promise<void> {
  return invoke("delete_backup", { backupPath });
}
