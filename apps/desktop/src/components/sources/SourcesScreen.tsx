import { useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { useSourceList, useAddSource, useScanSource } from "@/hooks/useSources";
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
}: {
  source: Source;
  onScan: (id: number) => void;
  scanning: boolean;
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
      <button
        onClick={() => onScan(source.id)}
        disabled={scanning || source.status === "offline"}
        className={clsx(
          "flex-shrink-0 px-3 py-1.5 text-xs rounded border transition-colors",
          scanning
            ? "border-mantis-700 text-mantis-600 animate-pulse cursor-wait"
            : source.status === "offline"
            ? "border-surface-border text-gray-700 cursor-not-allowed"
            : "border-mantis-700 text-mantis-400 hover:bg-mantis-700/20"
        )}
      >
        {scanning ? "スキャン中…" : "スキャン"}
      </button>
    </div>
  );
}

function AddSourceDialog({ onClose }: { onClose: () => void }) {
  const [name, setName] = useState("");
  const [path, setPath] = useState("");
  const { mutate: addSource, isPending } = useAddSource();

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
          <button
            onClick={() => setShowAdd(true)}
            className="flex items-center gap-1.5 px-3 py-1.5 text-sm bg-mantis-700 hover:bg-mantis-600 text-white rounded transition-colors"
          >
            <span>＋</span> ソースを追加
          </button>
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
              />
            ))}
          </div>
        )}

        {/* Info */}
        <div className="text-xs text-gray-700 border border-surface-border rounded p-3 leading-relaxed">
          <strong className="text-gray-500">NASオフライン耐性：</strong>
          NASがオフラインでも、既に登録されている作品はライブラリに表示され続けます。
          評価・タグ・メモは通常通り編集できます。
        </div>
      </div>

      {showAdd && <AddSourceDialog onClose={() => setShowAdd(false)} />}
    </div>
  );
}
