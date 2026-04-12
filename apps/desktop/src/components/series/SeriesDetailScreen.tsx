import { useState } from "react";
import { clsx } from "clsx";
import { useSeriesDetail, useSeriesWorks, useRemoveFromSeries, useDeleteSeries } from "@/hooks/useSeries";
import { useLibraryStore } from "@/store/libraryStore";
import { WorkCard } from "@/components/works/WorkCard";
import { StarRating } from "@/components/common/StarRating";
import type { WorkSummary } from "@cinemantis/shared-types";

const WATCH_STATUS_LABEL: Record<string, string> = {
  watched: "視聴済",
  watching: "視聴中",
  unwatched: "未視聴",
  skipped: "スキップ",
};

const SERIES_TYPE_LABELS: Record<string, string> = {
  movie_collection: "映画シリーズ",
  tv_show: "TVシリーズ",
  manual: "手動シリーズ",
};

export function SeriesDetailScreen({ seriesId }: { seriesId: number }) {
  const {
    setSelectedSeriesId,
    setSelectedWorkId,
    selectedWorkId,
    viewMode,
    setViewMode,
  } = useLibraryStore();

  const { data: series, isLoading: seriesLoading } = useSeriesDetail(seriesId);
  const { data: works = [], isLoading: worksLoading } = useSeriesWorks(seriesId);
  const { mutate: removeWork } = useRemoveFromSeries();
  const { mutate: deleteSeries } = useDeleteSeries();

  const [confirmDelete, setConfirmDelete] = useState(false);

  const isLoading = seriesLoading || worksLoading;
  const isManual = series?.series_type === "manual";

  function handleDeleteSeries() {
    deleteSeries(seriesId, {
      onSuccess: () => setSelectedSeriesId(null),
    });
  }

  if (isLoading) {
    return (
      <div className="flex-1 flex items-center justify-center text-gray-600 animate-pulse">
        読み込み中…
      </div>
    );
  }

  return (
    <div className="flex-1 flex flex-col min-h-0 overflow-hidden">
      {/* Header */}
      <div className="flex items-center gap-3 px-4 py-3 border-b border-subtle bg-surface-elevated flex-shrink-0">
        {/* Back */}
        <button
          onClick={() => setSelectedSeriesId(null)}
          className="text-gray-500 hover:text-gray-200 text-sm transition-colors flex items-center gap-1"
        >
          ← 一覧
        </button>

        <div className="w-px h-4 bg-surface-border" />

        <div className="flex-1 min-w-0">
          <h1 className="text-sm font-semibold text-gray-200 truncate">
            {series?.title ?? "シリーズ"}
          </h1>
          <div className="flex items-center gap-2 text-xs text-gray-600 mt-0.5">
            <span>{SERIES_TYPE_LABELS[series?.series_type ?? "manual"]}</span>
            <span>·</span>
            <span>{works.length} 作品</span>
            {series?.tmdb_id && (
              <>
                <span>·</span>
                <a
                  href={`https://www.themoviedb.org/${series.series_type === "tv_show" ? "tv" : "collection"}/${series.tmdb_id}`}
                  target="_blank"
                  rel="noopener noreferrer"
                  className="text-blue-500 hover:text-blue-400 transition-colors"
                >
                  TMDb #{series.tmdb_id}
                </a>
              </>
            )}
          </div>
        </div>

        {/* View mode */}
        <div className="flex border border-subtle rounded overflow-hidden">
          <button
            onClick={() => setViewMode("grid")}
            className={clsx(
              "px-2 py-1 text-sm transition-colors",
              viewMode === "grid"
                ? "bg-mantis-700/40 text-mantis-300"
                : "text-gray-500 hover:text-gray-300"
            )}
            title="グリッド表示"
          >
            ⊞
          </button>
          <button
            onClick={() => setViewMode("list")}
            className={clsx(
              "px-2 py-1 text-sm border-l border-subtle transition-colors",
              viewMode === "list"
                ? "bg-mantis-700/40 text-mantis-300"
                : "text-gray-500 hover:text-gray-300"
            )}
            title="リスト表示"
          >
            ≡
          </button>
        </div>

        {/* Delete (manual series only) */}
        {isManual && (
          <div className="relative">
            {confirmDelete ? (
              <div className="flex items-center gap-2">
                <span className="text-xs text-red-400">削除しますか?</span>
                <button
                  onClick={handleDeleteSeries}
                  className="text-xs text-red-500 hover:text-red-300 transition-colors"
                >
                  はい
                </button>
                <button
                  onClick={() => setConfirmDelete(false)}
                  className="text-xs text-gray-600 hover:text-gray-300 transition-colors"
                >
                  キャンセル
                </button>
              </div>
            ) : (
              <button
                onClick={() => setConfirmDelete(true)}
                className="text-xs text-gray-700 hover:text-red-500 transition-colors"
              >
                🗑 削除
              </button>
            )}
          </div>
        )}
      </div>

      {/* Overview */}
      {series?.overview && (
        <div className="px-4 py-2 border-b border-subtle flex-shrink-0">
          <p className="text-xs text-gray-500 leading-relaxed line-clamp-3">
            {series.overview}
          </p>
        </div>
      )}

      {/* Works */}
      <div className="flex-1 overflow-y-auto">
        {works.length === 0 ? (
          <div className="flex items-center justify-center h-40 text-gray-600 text-sm">
            作品がありません
          </div>
        ) : viewMode === "grid" ? (
          <div className="p-4">
            <div className="grid grid-cols-[repeat(auto-fill,minmax(130px,1fr))] gap-3">
              {works.map((work) => (
                <div key={work.id} className="relative group">
                  <WorkCard
                    work={work}
                    selected={selectedWorkId === work.id}
                    onSelect={() =>
                      setSelectedWorkId(selectedWorkId === work.id ? null : work.id)
                    }
                  />
                  {isManual && (
                    <button
                      onClick={(e) => {
                        e.stopPropagation();
                        removeWork({ seriesId, workId: work.id });
                      }}
                      className="absolute top-1 left-1 bg-black/70 text-white text-[10px] px-1 rounded opacity-0 group-hover:opacity-100 transition-opacity hover:bg-red-800"
                      title="シリーズから削除"
                    >
                      ✕
                    </button>
                  )}
                </div>
              ))}
            </div>
          </div>
        ) : (
          <table className="w-full text-sm border-collapse">
            <thead className="sticky top-0 bg-surface-elevated border-b border-subtle z-10">
              <tr>
                <th className="text-left px-4 py-2 text-gray-400 font-medium">タイトル</th>
                <th className="text-left px-3 py-2 text-gray-400 font-medium w-16">年</th>
                <th className="text-left px-3 py-2 text-gray-400 font-medium w-20">評価</th>
                <th className="text-left px-3 py-2 text-gray-400 font-medium w-16">視聴</th>
                <th className="text-left px-3 py-2 text-gray-400 font-medium w-24">状態</th>
                {isManual && (
                  <th className="w-10" />
                )}
              </tr>
            </thead>
            <tbody>
              {works.map((work) => (
                <SeriesWorkRow
                  key={work.id}
                  work={work}
                  selected={selectedWorkId === work.id}
                  onSelect={() =>
                    setSelectedWorkId(selectedWorkId === work.id ? null : work.id)
                  }
                  isManual={isManual}
                  onRemove={() => removeWork({ seriesId, workId: work.id })}
                />
              ))}
            </tbody>
          </table>
        )}
      </div>
    </div>
  );
}

// ─── シリーズ用の行コンポーネント（削除ボタン付き） ──────────────────────────

function SeriesWorkRow({
  work,
  selected,
  onSelect,
  isManual,
  onRemove,
}: {
  work: WorkSummary;
  selected: boolean;
  onSelect: () => void;
  isManual: boolean;
  onRemove: () => void;
}) {
  return (
    <tr
      onClick={onSelect}
      className={clsx(
        "cursor-pointer border-b border-subtle transition-colors",
        selected ? "bg-mantis-600/10 text-gray-100" : "hover:bg-surface-hover text-gray-300"
      )}
    >
      <td className="px-4 py-2">
        <div className="flex items-center gap-2">
          {work.isFavorite && <span className="text-xs">⭐</span>}
          <span className="font-medium">{work.title}</span>
        </div>
      </td>
      <td className="px-3 py-2 text-gray-500">{work.year ?? "—"}</td>
      <td className="px-3 py-2">
        {work.userRating !== null ? (
          <StarRating value={work.userRating} size="xs" readonly />
        ) : (
          <span className="text-gray-600">—</span>
        )}
      </td>
      <td className="px-3 py-2 text-gray-500">{work.playCount}</td>
      <td className="px-3 py-2">
        <span
          className={clsx(
            "text-xs px-1.5 py-0.5 rounded",
            work.watchStatus === "watched" && "bg-mantis-900/60 text-mantis-400",
            work.watchStatus === "watching" && "bg-blue-900/60 text-blue-400",
            work.watchStatus === "unwatched" && "bg-surface-border text-gray-500",
          )}
        >
          {WATCH_STATUS_LABEL[work.watchStatus] ?? work.watchStatus}
        </span>
      </td>
      {isManual && (
        <td className="px-2 py-1.5 w-10">
          <button
            onClick={(e) => { e.stopPropagation(); onRemove(); }}
            className="text-gray-700 hover:text-red-500 text-xs transition-colors"
            title="シリーズから削除"
          >
            ✕
          </button>
        </td>
      )}
    </tr>
  );
}
