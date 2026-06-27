import { useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { useSourceList, useAddSource, useDeduplicateLibraryFiles, useDeleteSource, useScanSource } from "@/hooks/useSources";
import { useProcessSyncOutbox, useSyncOutboxList } from "@/hooks/useSyncOutbox";
import type { Source } from "@cinemantis/shared-types";
import { clsx } from "clsx";

const STATUS_CLASS: Record<string, string> = {
  online: "text-mantis-400",
  offline: "text-gray-600",
  error: "text-red-500",
};

const STATUS_LABEL: Record<string, string> = {
  online: "オンライン",
  offline: "オフライン",
  error: "エラー",
};

function SourceCard({
  source,
  onScan,
  scanning,
  onDelete,
  deleting,
}: {
  source: Source;
  onScan: (id: number) => void;
  scanning: boolean;
  onDelete: (source: Source) => void;
  deleting: boolean;
}) {
  return (
    <div className="flex items-start gap-3 p-4 bg-surface rounded border border-subtle">
      {/* Icon */}
      <div className="text-2xl flex-shrink-0 mt-0.5">
        📁
      </div>

      {/* Info */}
      <div className="flex-1 min-w-0">
        <div className="flex items-center gap-2">
          <span className="font-medium text-gray-200">{source.name}</span>
          <span className={clsx("text-xs", STATUS_CLASS[source.status] ?? "text-gray-500")}>
            ● {STATUS_LABEL[source.status] ?? source.status}
          </span>
        </div>
        <p className="text-xs text-gray-500 truncate mt-0.5">{source.rootPath}</p>
        <div className="flex items-center gap-3 mt-1.5 text-xs text-gray-600">
          {source.lastScanAt && (
            <span>
              最終スキャン: {new Date(source.lastScanAt).toLocaleString("ja-JP")}
            </span>
          )}
        </div>
      </div>

      {/* Actions */}
      <div className="flex flex-shrink-0 items-center gap-2">
        <button
          onClick={() => onScan(source.id)}
          disabled={scanning || source.status === "offline"}
          className={clsx(
            "px-3 py-1.5 text-xs rounded border transition-colors",
            scanning
              ? "border-mantis-700 text-mantis-600 animate-pulse cursor-wait"
              : source.status === "offline"
              ? "border-surface-border text-gray-700 cursor-not-allowed"
              : "border-mantis-700 text-mantis-400 hover:bg-mantis-700/20"
          )}
        >
          {scanning ? "スキャン中…" : "スキャン"}
        </button>
        <button
          onClick={() => onDelete(source)}
          disabled={scanning || deleting}
          className="px-2 py-1.5 text-xs text-red-500 border border-red-900/70 hover:bg-red-950/30 disabled:opacity-30"
          title="ソース登録と、このソースだけに属するライブラリ項目を削除"
        >
          削除
        </button>
      </div>
    </div>
  );
}

function AddSourceDialog({ onClose }: { onClose: () => void }) {
  const [name, setName] = useState("");
  const [path, setPath] = useState("");
  const { mutate: addSource, isPending, error } = useAddSource();

  async function pickFolder() {
    const selected = await open({ directory: true, multiple: false });
    if (typeof selected === "string") {
      setPath(selected);
      if (!name) {
        // Auto-fill name from last folder segment
        setName(selected.split(/[\\/]/).filter(Boolean).pop() ?? "");
      }
    }
  }

  function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    if (!path) return;
    addSource(
      { name: name || path, root_path: path, source_type: "local", media_kind: "unknown" },
      { onSuccess: onClose }
    );
  }

  return (
    <div className="fixed inset-0 bg-black/60 flex items-center justify-center z-50">
      <form
        onSubmit={handleSubmit}
        className="bg-surface-elevated border border-subtle rounded-lg p-6 w-[420px] flex flex-col gap-4 shadow-xl"
      >
        <h2 className="text-base font-semibold text-gray-100">ソースを追加</h2>
        {error && <div className="text-xs text-red-400">{String(error)}</div>}

        {/* Path */}
        <div className="flex flex-col gap-1.5">
          <label className="text-xs text-gray-400">フォルダパス</label>
          <div className="flex gap-2">
            <input
              type="text"
              value={path}
              onChange={(e) => setPath(e.target.value)}
              placeholder="C:\Movies"
              className="flex-1 bg-surface border border-subtle rounded px-3 py-1.5 text-sm text-gray-200 placeholder-gray-700 outline-none focus:border-mantis-600"
              required
            />
            <button
              type="button"
              onClick={pickFolder}
              className="px-3 py-1.5 text-sm border border-subtle rounded text-gray-400 hover:text-gray-200 hover:border-gray-500 transition-colors"
            >
              参照
            </button>
          </div>
        </div>

        {/* Name */}
        <div className="flex flex-col gap-1.5">
          <label className="text-xs text-gray-400">名前</label>
          <input
            type="text"
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="マイ映画フォルダ"
            className="bg-surface border border-subtle rounded px-3 py-1.5 text-sm text-gray-200 placeholder-gray-700 outline-none focus:border-mantis-600"
          />
        </div>

        {/* Actions */}
        <div className="flex justify-end gap-2 pt-1">
          <button
            type="button"
            onClick={onClose}
            className="px-4 py-1.5 text-sm text-gray-400 hover:text-gray-200 transition-colors"
          >
            キャンセル
          </button>
          <button
            type="submit"
            disabled={!path || isPending}
            className="px-4 py-1.5 text-sm bg-mantis-700 hover:bg-mantis-600 text-white rounded transition-colors disabled:opacity-50"
          >
            {isPending ? "追加中…" : "追加"}
          </button>
        </div>
      </form>
    </div>
  );
}

export function SourcesScreen() {
  const { data: sources = [], isLoading } = useSourceList();
  const {
    mutate: scanSource,
    variables: scanningId,
    isPending: scanning,
    error: scanError,
    isError: isScanError,
    data: scanResult,
    isSuccess: isScanSuccess,
  } = useScanSource();
  const [showAdd, setShowAdd] = useState(false);
  const { mutate: deleteSource, variables: deletingId, isPending: deleting } = useDeleteSource();
  const { mutate: deduplicate, data: deduplicateResult, isPending: deduplicating, error: deduplicateError } = useDeduplicateLibraryFiles();
  const { data: syncItems = [] } = useSyncOutboxList();
  const { mutate: processSync, data: syncResult, isPending: syncing, error: syncError } = useProcessSyncOutbox();
  const pendingDeletes = syncItems.filter((item) => item.action_type === "delete_file");

  function confirmDelete(source: Source) {
    const ok = window.confirm(
      `ソース「${source.name}」を削除します。\n\n元の動画ファイルは削除しません。このソースだけに属するライブラリ項目はDBから削除されます。`,
    );
    if (ok) deleteSource(source.id);
  }

  return (
    <div className="flex-1 overflow-y-auto p-6">
      <div className="max-w-2xl mx-auto flex flex-col gap-6">
        {/* Header */}
        <div className="flex items-center justify-between">
          <div>
            <h1 className="text-lg font-semibold text-gray-100">ソース管理</h1>
            <p className="text-xs text-gray-500 mt-0.5">
              動画ファイルの参照元フォルダを管理します
            </p>
          </div>
          <div className="flex items-center gap-2">
            <button
              onClick={() => processSync(null)}
              disabled={!pendingDeletes.length || syncing}
              className={clsx(
                "px-3 py-1.5 text-sm border transition-colors",
                pendingDeletes.length
                  ? "border-yellow-700 text-yellow-300 hover:bg-yellow-900/20"
                  : "border-gray-800 text-gray-700 cursor-not-allowed",
              )}
              title="NAS復帰後に保留中の実ファイル操作を反映"
            >
              {syncing ? "同期中…" : `同期待ち ${pendingDeletes.length} 件`}
            </button>
            <button
              onClick={() => deduplicate()}
              disabled={deduplicating}
              className="px-3 py-1.5 text-sm border border-gray-700 text-gray-400 hover:text-gray-200 disabled:opacity-40"
              title="同じ実ファイルから作られた重複作品を整理"
            >
              {deduplicating ? "整理中…" : "重複を整理"}
            </button>
            <button
              onClick={() => setShowAdd(true)}
              className="flex items-center gap-1.5 px-3 py-1.5 text-sm bg-mantis-700 hover:bg-mantis-600 text-white rounded transition-colors"
            >
              <span>＋</span> ソースを追加
            </button>
          </div>
        </div>

        {/* Scan result / error banner */}
        {isScanError && (
          <div className="text-xs text-red-400 bg-red-900/20 border border-red-800 rounded px-3 py-2">
            スキャン失敗: {String(scanError)}
          </div>
        )}
        {isScanSuccess && scanResult && (
          <div className="text-xs text-mantis-400 bg-mantis-900/20 border border-mantis-800 rounded px-3 py-2">
            スキャン完了 — 新規: {scanResult.new_files} 件 / 更新: {scanResult.updated_files} 件 / 不明: {scanResult.missing_files} 件
          </div>
        )}
        {deduplicateResult && (
          <div className="text-xs text-mantis-400 bg-mantis-900/20 border border-mantis-800 rounded px-3 py-2">
            重複整理完了 — ファイル: {deduplicateResult.removed_files}件 / 作品: {deduplicateResult.removed_works}件を整理
          </div>
        )}
        {deduplicateError && <div className="text-xs text-red-400">重複整理失敗: {String(deduplicateError)}</div>}
        {syncResult && (
          <div className="text-xs text-mantis-400 bg-mantis-900/20 border border-mantis-800 rounded px-3 py-2">
            同期完了 — 削除: {syncResult.deleted} 件 / 既に存在なし: {syncResult.already_missing} 件 / オフライン: {syncResult.skipped_offline} 件 / 失敗: {syncResult.failed} 件
          </div>
        )}
        {syncError && <div className="text-xs text-red-400">同期失敗: {String(syncError)}</div>}
        {pendingDeletes.length > 0 && (
          <div className="rounded border border-yellow-900/50 bg-yellow-950/10 p-3 text-xs text-yellow-200/80">
            <div className="mb-2 font-medium text-yellow-300">
              NAS復帰待ちの実ファイル削除が {pendingDeletes.length} 件あります
            </div>
            <div className="grid gap-1 text-gray-500">
              {pendingDeletes.slice(0, 4).map((item) => (
                <div key={item.id} className="truncate">
                  {item.work_title ?? item.target_path}
                </div>
              ))}
              {pendingDeletes.length > 4 && <div>ほか {pendingDeletes.length - 4} 件</div>}
            </div>
          </div>
        )}

        {/* Source list */}
        {isLoading ? (
          <div className="text-gray-600 text-sm animate-pulse">読み込み中…</div>
        ) : sources.length === 0 ? (
          <div className="flex flex-col items-center justify-center py-16 gap-3 text-gray-600">
            <span className="text-4xl opacity-30">📁</span>
            <p className="text-sm">ソースがありません</p>
            <button
              onClick={() => setShowAdd(true)}
              className="text-xs text-mantis-500 hover:text-mantis-400 underline underline-offset-2"
            >
              最初のソースを追加する
            </button>
          </div>
        ) : (
          <div className="flex flex-col gap-2">
            {sources.map((source) => (
              <SourceCard
                key={source.id}
                source={source}
                onScan={(id) => scanSource(id)}
                scanning={scanning && scanningId === source.id}
                onDelete={confirmDelete}
                deleting={deleting && deletingId === source.id}
              />
            ))}
          </div>
        )}

        {/* Info */}
        <div className="text-xs text-gray-700 border border-surface-border rounded p-3 leading-relaxed">
          <strong className="text-gray-500">NASオフライン耐性：</strong>
          NASがオフラインでも、既に登録されている作品はライブラリに表示され続けます。
          評価などのDBメタデータは通常通り編集できます。オフライン中の実ファイル削除は同期待ちに入り、NAS復帰後に反映できます。
        </div>
      </div>

      {showAdd && <AddSourceDialog onClose={() => setShowAdd(false)} />}
    </div>
  );
}
