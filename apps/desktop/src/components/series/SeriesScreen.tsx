import { useState } from "react";
import { clsx } from "clsx";
import { convertFileSrc } from "@tauri-apps/api/core";
import { useSeriesList, useCreateSeries, useDeleteSeries, useSyncMovieCollections } from "@/hooks/useSeries";
import { useLibraryStore } from "@/store/libraryStore";
import type { SeriesSummary } from "@/api/series";

// ─── ユーティリティ ───────────────────────────────────────────────────────────

function toAssetUrl(path: string): string {
  return convertFileSrc(path);
}

const SERIES_TYPE_LABELS: Record<string, { label: string; color: string }> = {
  movie_collection: { label: "映画シリーズ", color: "text-blue-400" },
  tv_show:          { label: "TVシリーズ",   color: "text-purple-400" },
  manual:           { label: "手動",         color: "text-gray-500" },
};

// ─── シリーズカード ───────────────────────────────────────────────────────────

function SeriesCard({
  series,
  onClick,
}: {
  series: SeriesSummary;
  onClick: () => void;
}) {
  const [imgFailed, setImgFailed] = useState(false);
  const typeInfo = SERIES_TYPE_LABELS[series.series_type] ?? SERIES_TYPE_LABELS.manual;

  return (
    <div
      onClick={onClick}
      className="group cursor-pointer flex flex-col gap-1.5"
    >
      {/* Poster */}
      <div className="relative aspect-[2/3] bg-surface rounded overflow-hidden">
        {series.poster_path && !imgFailed ? (
          <img
            src={toAssetUrl(series.poster_path)}
            alt={series.title}
            className="w-full h-full object-cover transition-transform duration-200 group-hover:scale-105"
            onError={() => setImgFailed(true)}
          />
        ) : (
          <div className="w-full h-full flex items-center justify-center bg-surface-hover">
            <span className="text-4xl opacity-20">
              {series.series_type === "tv_show" ? "📺" : "🎬"}
            </span>
          </div>
        )}

        {/* Work count badge */}
        <div className="absolute bottom-1.5 right-1.5 bg-black/70 text-white text-xs px-1.5 py-0.5 rounded font-mono">
          {series.work_count}
        </div>
      </div>

      {/* Info */}
      <div className="px-0.5">
        <p className="text-xs font-medium text-gray-200 leading-snug line-clamp-2">
          {series.title}
        </p>
        <p className={clsx("text-[10px] mt-0.5", typeInfo.color)}>
          {typeInfo.label}
        </p>
      </div>
    </div>
  );
}

// ─── メイン画面 ──────────────────────────────────────────────────────────────

export function SeriesScreen() {
  const { setSelectedSeriesId } = useLibraryStore();
  const { data: seriesList = [], isLoading, isError, error } = useSeriesList();
  const { mutate: createSeries, isPending: creating } = useCreateSeries();
  const { mutate: syncCollections, data: syncResult, isPending: syncing, error: syncError } = useSyncMovieCollections();
  useDeleteSeries();

  const [newTitle, setNewTitle] = useState("");
  const [showNewForm, setShowNewForm] = useState(false);
  function handleCreate() {
    const t = newTitle.trim();
    if (!t) return;
    createSeries(
      { title: t, seriesType: "manual" },
      {
        onSuccess: () => {
          setNewTitle("");
          setShowNewForm(false);
        },
      }
    );
  }

  return (
    <div className="flex-1 flex flex-col min-h-0 overflow-hidden">
      {/* Header */}
      <div className="flex items-center justify-between px-4 py-3 border-b border-subtle flex-shrink-0 bg-surface-elevated">
        <div className="flex items-center gap-3">
          <h1 className="text-sm font-semibold text-gray-200">シリーズ</h1>
          <span className="text-xs text-gray-600">{seriesList.length} 件</span>
        </div>

        <div className="flex items-center gap-2">
          {syncResult && <span className="text-[11px] text-gray-500">{syncResult.succeeded}/{syncResult.total}作品をシリーズ化</span>}
          {syncError && <span className="max-w-64 truncate text-[11px] text-red-400">{String(syncError)}</span>}
          <button
            onClick={() => syncCollections()}
            disabled={syncing}
            className="border border-mantis-800/60 px-2.5 py-1 text-xs text-mantis-400 hover:bg-mantis-900/20 disabled:opacity-40"
          >
            {syncing ? "TMDbから同期中…" : "TMDbからシリーズ同期"}
          </button>
          <button
            onClick={() => setShowNewForm((v) => !v)}
            className="text-xs text-gray-500 hover:text-mantis-400 transition-colors px-2 py-1 border border-subtle"
          >
            ＋ 手動シリーズ
          </button>
        </div>
      </div>

      {/* New series form */}
      {showNewForm && (
        <div className="flex items-center gap-2 px-4 py-2 bg-surface border-b border-subtle flex-shrink-0">
          <input
            type="text"
            value={newTitle}
            onChange={(e) => setNewTitle(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && handleCreate()}
            placeholder="シリーズ名…"
            autoFocus
            className="flex-1 max-w-sm bg-surface-hover border border-subtle rounded px-3 py-1 text-sm text-gray-200 placeholder-gray-600 outline-none focus:border-mantis-600"
          />
          <button
            onClick={handleCreate}
            disabled={!newTitle.trim() || creating}
            className="px-3 py-1 text-xs bg-mantis-700 hover:bg-mantis-600 text-white rounded transition-colors disabled:opacity-40"
          >
            作成
          </button>
          <button
            onClick={() => { setShowNewForm(false); setNewTitle(""); }}
            className="text-xs text-gray-600 hover:text-gray-300"
          >
            キャンセル
          </button>
        </div>
      )}

      {/* Content */}
      <div className="flex-1 overflow-y-auto p-4">
        {isLoading ? (
          <div className="flex items-center justify-center h-40 text-gray-600 animate-pulse">
            読み込み中…
          </div>
        ) : isError ? (
          <div className="flex flex-col items-center justify-center h-40 gap-2 text-red-500 text-sm">
            <span>読み込みに失敗しました</span>
            <span className="max-w-xl break-all text-center text-xs text-red-700">{String(error)}</span>
          </div>
        ) : seriesList.length === 0 ? (
          <div className="flex flex-col items-center justify-center h-40 gap-3 text-gray-600">
            <span className="text-4xl opacity-20">🗂</span>
            <p className="text-sm">
              シリーズがありません
            </p>
            {seriesList.length === 0 && (
              <p className="text-xs text-gray-700 text-center max-w-xs">
                TMDbコレクションから自動作成するか、手動で作品をまとめられます
              </p>
            )}
          </div>
        ) : (
          <div className="grid grid-cols-[repeat(auto-fill,minmax(130px,1fr))] gap-4">
            {seriesList.map((series) => (
              <SeriesCard
                key={series.id}
                series={series}
                onClick={() => setSelectedSeriesId(series.id)}
              />
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
