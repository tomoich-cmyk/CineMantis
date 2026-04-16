import { useState } from "react";
import { useBackupList, useCreateBackup, useRestoreDatabase, useDeleteBackup } from "@/hooks/useBackup";
import type { BackupEntry } from "@/api/backup";

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(2)} MB`;
}

function formatDate(iso: string): string {
  if (!iso) return "—";
  try {
    return new Date(iso).toLocaleString("ja-JP", {
      year: "numeric", month: "short", day: "numeric",
      hour: "2-digit", minute: "2-digit",
    });
  } catch {
    return iso;
  }
}

export function BackupScreen() {
  const { data: backups = [], isLoading } = useBackupList();
  const { mutate: createBackup, isPending: creating, data: lastBackupPath, error: createError, isSuccess: createSuccess } = useCreateBackup();
  const { mutate: restoreDb, isPending: restoring, isSuccess: restoreSuccess } = useRestoreDatabase();
  const { mutate: deleteEntry, isPending: deleting } = useDeleteBackup();

  const [labelInput, setLabelInput]     = useState("");
  const [confirmRestore, setConfirmRestore] = useState<BackupEntry | null>(null);
  const [confirmDelete, setConfirmDelete]   = useState<BackupEntry | null>(null);

  return (
    <div className="flex-1 overflow-y-auto p-6">
      <div className="max-w-xl mx-auto flex flex-col gap-8">
        <div>
          <h1 className="text-lg font-semibold text-gray-100">バックアップ</h1>
          <p className="text-xs text-gray-500 mt-0.5">
            評価・視聴状態・タグ・照合結果など、すべての記録を DB ファイルとして保存します
          </p>
        </div>

        {/* ── バックアップ作成 ── */}
        <section className="flex flex-col gap-3">
          <h2 className="text-sm font-medium text-gray-200">バックアップを作成</h2>

          <div className="flex gap-2">
            <input
              type="text"
              value={labelInput}
              onChange={(e) => setLabelInput(e.target.value)}
              placeholder="ラベル（任意）"
              className="flex-1 bg-surface border border-subtle rounded px-3 py-1.5 text-sm text-gray-200 placeholder-gray-600 outline-none focus:border-mantis-600 transition-colors"
            />
            <button
              onClick={() => {
                createBackup(labelInput || undefined);
                setLabelInput("");
              }}
              disabled={creating}
              className="px-4 py-1.5 text-sm bg-mantis-700/30 border border-mantis-700/60 text-mantis-300 rounded hover:bg-mantis-700/50 transition-colors disabled:opacity-40"
            >
              {creating ? "作成中…" : "今すぐバックアップ"}
            </button>
          </div>

          {/* 成功メッセージ */}
          {createSuccess && lastBackupPath && (
            <div className="text-xs text-mantis-400 bg-mantis-900/20 border border-mantis-800/40 rounded px-3 py-2">
              ✓ バックアップ作成完了:
              <span className="font-mono text-gray-400 ml-1 break-all">{lastBackupPath}</span>
            </div>
          )}
          {createError && (
            <div className="text-xs text-red-400 bg-red-900/20 border border-red-800/40 rounded px-3 py-2">
              エラー: {String(createError)}
            </div>
          )}
        </section>

        {/* ── バックアップ一覧 ── */}
        <section className="flex flex-col gap-3">
          <h2 className="text-sm font-medium text-gray-200">
            バックアップ一覧
            <span className="ml-2 text-xs text-gray-600 font-normal">{backups.length} 件</span>
          </h2>

          {isLoading ? (
            <p className="text-sm text-gray-600 animate-pulse">読み込み中…</p>
          ) : backups.length === 0 ? (
            <p className="text-sm text-gray-600">バックアップがありません</p>
          ) : (
            <div className="flex flex-col gap-2">
              {backups.map((entry) => (
                <div
                  key={entry.path}
                  className="flex items-center gap-3 px-3 py-2.5 bg-surface-elevated border border-subtle rounded"
                >
                  <div className="flex-1 min-w-0">
                    <p className="text-xs font-medium text-gray-200 truncate">{entry.name}</p>
                    <p className="text-[11px] text-gray-600 mt-0.5">
                      {formatDate(entry.created_at)} · {formatBytes(entry.size_bytes)}
                    </p>
                  </div>
                  <div className="flex gap-1.5">
                    <button
                      onClick={() => setConfirmRestore(entry)}
                      disabled={restoring || deleting}
                      className="text-xs px-2.5 py-1 border border-yellow-800/60 text-yellow-600 rounded hover:text-yellow-400 hover:border-yellow-600 transition-colors disabled:opacity-30"
                    >
                      復元
                    </button>
                    <button
                      onClick={() => setConfirmDelete(entry)}
                      disabled={deleting}
                      className="text-xs px-2 py-1 text-gray-700 hover:text-red-500 transition-colors disabled:opacity-30"
                      title="削除"
                    >
                      ×
                    </button>
                  </div>
                </div>
              ))}
            </div>
          )}
        </section>

        {/* 復元後のメッセージ */}
        {restoreSuccess && (
          <div className="text-sm text-yellow-400 bg-yellow-900/20 border border-yellow-800/40 rounded px-4 py-3">
            <p className="font-medium">⚠ 復元の準備ができました</p>
            <p className="text-xs mt-1 text-yellow-600">
              アプリを再起動すると、選択したバックアップが適用されます。
            </p>
          </div>
        )}

        {/* 注意事項 */}
        <section className="text-xs text-gray-700 leading-relaxed">
          <p className="font-medium text-gray-500 mb-1">バックアップについて</p>
          <ul className="list-disc list-inside space-y-1">
            <li>評価・視聴状態・タグ・照合結果・メモがすべて含まれます</li>
            <li>動画ファイル自体はバックアップされません</li>
            <li>サムネイル・ポスター画像は再生成可能なため含まれません</li>
            <li>復元後はアプリの再起動が必要です</li>
          </ul>
        </section>
      </div>

      {/* ── 復元確認ダイアログ ── */}
      {confirmRestore && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60">
          <div role="dialog" className="bg-surface-elevated border border-subtle rounded-lg p-6 w-80 shadow-2xl">
            <h3 className="text-sm font-semibold text-gray-100 mb-2">復元の確認</h3>
            <p className="text-xs text-gray-400 mb-1">以下のバックアップから復元します：</p>
            <p className="text-xs font-mono text-gray-300 bg-surface rounded px-2 py-1 mb-4 break-all">
              {confirmRestore.name}
            </p>
            <p className="text-xs text-yellow-500 mb-4">
              ⚠ 現在の記録（評価・視聴状態等）は上書きされます。
              復元後はアプリを再起動してください。
            </p>
            <div className="flex gap-2 justify-end">
              <button
                onClick={() => setConfirmRestore(null)}
                className="px-3 py-1.5 text-xs border border-subtle rounded text-gray-400 hover:text-gray-200 transition-colors"
              >
                キャンセル
              </button>
              <button
                onClick={() => {
                  restoreDb(confirmRestore.path);
                  setConfirmRestore(null);
                }}
                className="px-3 py-1.5 text-xs bg-yellow-800/40 border border-yellow-700/60 rounded text-yellow-300 hover:bg-yellow-800/60 transition-colors"
              >
                復元する
              </button>
            </div>
          </div>
        </div>
      )}

      {/* ── 削除確認ダイアログ ── */}
      {confirmDelete && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/60">
          <div role="dialog" className="bg-surface-elevated border border-subtle rounded-lg p-6 w-80 shadow-2xl">
            <h3 className="text-sm font-semibold text-gray-100 mb-2">バックアップを削除</h3>
            <p className="text-xs text-gray-400 mb-3">
              <span className="font-mono text-gray-300">{confirmDelete.name}</span> を削除しますか？
            </p>
            <div className="flex gap-2 justify-end">
              <button
                onClick={() => setConfirmDelete(null)}
                className="px-3 py-1.5 text-xs border border-subtle rounded text-gray-400 hover:text-gray-200 transition-colors"
              >
                キャンセル
              </button>
              <button
                onClick={() => {
                  deleteEntry(confirmDelete.path);
                  setConfirmDelete(null);
                }}
                className="px-3 py-1.5 text-xs bg-red-900/40 border border-red-700/60 rounded text-red-300 hover:bg-red-900/60 transition-colors"
              >
                削除
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
