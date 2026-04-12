import { useState } from "react";
import { clsx } from "clsx";
import { useTmdbCandidates, useApplyTmdbMatch } from "@/hooks/useTmdb";
import { tmdbPosterUrl, type TmdbCandidate } from "@/api/tmdb";

interface Props {
  workId: number;
  workTitle: string;
  onClose: () => void;
}

function CandidateCard({
  c,
  selected,
  onClick,
}: {
  c: TmdbCandidate;
  selected: boolean;
  onClick: () => void;
}) {
  const posterUrl = c.poster_path ? tmdbPosterUrl(c.poster_path) : null;

  return (
    <div
      onClick={onClick}
      className={clsx(
        "flex gap-3 p-3 rounded cursor-pointer border transition-all",
        selected
          ? "border-mantis-500 bg-mantis-900/20"
          : "border-surface-border hover:border-gray-600 bg-surface"
      )}
    >
      {/* Poster */}
      <div className="w-12 h-18 flex-shrink-0 rounded overflow-hidden bg-surface-hover flex items-center justify-center">
        {posterUrl ? (
          <img
            src={posterUrl}
            alt={c.title}
            className="w-full h-full object-cover"
            style={{ minHeight: "72px" }}
          />
        ) : (
          <span className="text-xl opacity-20">🎬</span>
        )}
      </div>

      {/* Info */}
      <div className="flex-1 min-w-0 flex flex-col gap-1">
        <div className="flex items-start justify-between gap-2">
          <div>
            <p className="text-sm font-medium text-gray-100 leading-tight">
              {c.title}
            </p>
            {c.original_title && c.original_title !== c.title && (
              <p className="text-xs text-gray-500 italic">{c.original_title}</p>
            )}
          </div>
          {/* Confidence badge */}
          <span
            className={clsx(
              "flex-shrink-0 text-xs px-1.5 py-0.5 rounded font-mono",
              c.confidence >= 90
                ? "bg-mantis-900/60 text-mantis-400"
                : c.confidence >= 70
                ? "bg-yellow-900/60 text-yellow-400"
                : "bg-surface-border text-gray-500"
            )}
          >
            {c.confidence}
          </span>
        </div>

        <div className="flex items-center gap-2 text-xs text-gray-500">
          <span
            className={clsx(
              "px-1 rounded",
              c.media_type === "movie" ? "bg-blue-900/40 text-blue-400" : "bg-purple-900/40 text-purple-400"
            )}
          >
            {c.media_type === "movie" ? "映画" : "TV"}
          </span>
          {c.year && <span>{c.year}</span>}
        </div>

        {c.overview && (
          <p className="text-xs text-gray-600 line-clamp-2 leading-relaxed">
            {c.overview}
          </p>
        )}

        {/* Reasons */}
        <div className="flex flex-wrap gap-1 mt-0.5">
          {c.reasons.map((r) => (
            <span
              key={r}
              className="text-[10px] px-1 rounded bg-surface-border text-gray-600"
            >
              {r}
            </span>
          ))}
        </div>
      </div>
    </div>
  );
}

export function CandidateDialog({ workId, workTitle, onClose }: Props) {
  const { data: candidates = [], isLoading, isError } = useTmdbCandidates(workId);
  const { mutate: applyMatch, isPending: applying } = useApplyTmdbMatch();

  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [lock, setLock] = useState(false);

  const selected = candidates.find((c) => c.tmdb_id === selectedId) ?? null;

  function handleApply() {
    if (!selected) return;
    applyMatch(
      {
        workId,
        tmdbId: selected.tmdb_id,
        mediaType: selected.media_type,
        lock,
      },
      { onSuccess: onClose }
    );
  }

  return (
    <div className="fixed inset-0 bg-black/70 flex items-center justify-center z-50 p-4">
      <div className="bg-surface-elevated border border-subtle rounded-lg w-full max-w-lg max-h-[85vh] flex flex-col shadow-2xl">
        {/* Header */}
        <div className="flex items-center justify-between px-5 py-4 border-b border-subtle flex-shrink-0">
          <div>
            <h2 className="text-sm font-semibold text-gray-100">TMDb 候補</h2>
            <p className="text-xs text-gray-500 mt-0.5 truncate max-w-[320px]">
              {workTitle}
            </p>
          </div>
          <button
            onClick={onClose}
            className="text-gray-600 hover:text-gray-300 text-xl leading-none"
          >
            ×
          </button>
        </div>

        {/* Content */}
        <div className="flex-1 overflow-y-auto p-4">
          {isLoading ? (
            <div className="flex items-center justify-center py-12 text-gray-600 text-sm animate-pulse">
              TMDb を検索中…
            </div>
          ) : isError ? (
            <div className="text-red-500 text-sm py-6 text-center">
              検索に失敗しました。APIキーを確認してください。
            </div>
          ) : candidates.length === 0 ? (
            <div className="text-gray-600 text-sm py-6 text-center">
              候補が見つかりませんでした
            </div>
          ) : (
            <div className="flex flex-col gap-2">
              {candidates.map((c) => (
                <CandidateCard
                  key={`${c.tmdb_id}-${c.media_type}`}
                  c={c}
                  selected={selectedId === c.tmdb_id}
                  onClick={() =>
                    setSelectedId(selectedId === c.tmdb_id ? null : c.tmdb_id)
                  }
                />
              ))}
            </div>
          )}
        </div>

        {/* Footer */}
        <div className="flex items-center justify-between px-5 py-3 border-t border-subtle flex-shrink-0">
          <label className="flex items-center gap-2 text-xs text-gray-400 cursor-pointer">
            <input
              type="checkbox"
              checked={lock}
              onChange={(e) => setLock(e.target.checked)}
              className="accent-mantis-500"
            />
            この候補で固定する（再スキャンで変更しない）
          </label>
          <div className="flex gap-2">
            <button
              onClick={onClose}
              className="px-3 py-1.5 text-xs text-gray-400 hover:text-gray-200 transition-colors"
            >
              キャンセル
            </button>
            <button
              onClick={handleApply}
              disabled={!selected || applying}
              className="px-4 py-1.5 text-xs bg-mantis-700 hover:bg-mantis-600 text-white rounded transition-colors disabled:opacity-40"
            >
              {applying ? "適用中…" : "この候補を適用"}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
