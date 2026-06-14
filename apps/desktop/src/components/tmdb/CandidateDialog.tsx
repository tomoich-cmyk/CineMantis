import { useState } from "react";
import { clsx } from "clsx";
import { useTmdbCandidates, useApplyTmdbMatch, type CandidateSearchParams } from "@/hooks/useTmdb";
import { tmdbPosterUrl, type TmdbCandidate } from "@/api/tmdb";

interface Props {
  workId: number;
  workTitle: string;
  isLocked?: boolean;
  onClose: () => void;
}

// ─── 候補カード ───────────────────────────────────────────────────────────────

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
  const [posterFailed, setPosterFailed] = useState(false);

  return (
    <div
      onClick={onClick}
      className={clsx(
        "flex gap-4 p-3 rounded cursor-pointer border transition-all",
        selected
          ? "border-mantis-500 bg-mantis-900/20"
          : "border-surface-border hover:border-gray-600 bg-surface"
      )}
    >
      {/* Poster */}
      <div className="w-24 aspect-[2/3] flex-shrink-0 rounded overflow-hidden bg-surface-hover flex items-center justify-center">
        {posterUrl && !posterFailed ? (
          <img
            src={posterUrl}
            alt={c.title}
            className="w-full h-full object-cover"
            onError={() => setPosterFailed(true)}
          />
        ) : (
          <span className="text-xs text-gray-700 text-center px-2">画像なし</span>
        )}
      </div>

      {/* Info */}
      <div className="flex-1 min-w-0 flex flex-col gap-1">
        <div className="flex items-start justify-between gap-2">
          <div className="min-w-0">
            <p className="text-sm font-medium text-gray-100 leading-tight truncate">
              {c.title}
            </p>
            {c.original_title && c.original_title !== c.title && (
              <p className="text-xs text-gray-500 italic truncate">{c.original_title}</p>
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
              c.media_type === "movie"
                ? "bg-blue-900/40 text-blue-400"
                : "bg-purple-900/40 text-purple-400"
            )}
          >
            {c.media_type === "movie" ? "映画" : "TV"}
          </span>
          {c.year && <span>{c.year}</span>}
          <a
            href={`https://www.themoviedb.org/${c.media_type}/${c.tmdb_id}`}
            target="_blank"
            rel="noopener noreferrer"
            className="text-gray-600 hover:text-blue-400 transition-colors"
            onClick={(e) => e.stopPropagation()}
          >
            #{c.tmdb_id}
          </a>
        </div>

        {c.overview && (
          <p className="text-xs text-gray-600 line-clamp-3 leading-relaxed">
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

// ─── メインダイアログ ──────────────────────────────────────────────────────────

export function CandidateDialog({ workId, workTitle, isLocked = false, onClose }: Props) {
  // 検索パラメータ（入力中の値）
  const [draftQuery, setDraftQuery] = useState(workTitle);
  const [draftMediaType, setDraftMediaType] = useState<"" | "movie" | "tv">("");

  // 確定済みの検索パラメータ（検索ボタン押下で更新）
  const [searchParams, setSearchParams] = useState<CandidateSearchParams>({
    queryOverride: null,
    mediaTypeHint: null,
  });

  // 候補選択
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [selectedMediaType, setSelectedMediaType] = useState<string>("");

  // 直接 TMDb ID 指定
  const [showDirectId, setShowDirectId] = useState(false);
  const [directIdInput, setDirectIdInput] = useState("");
  const [directMediaType, setDirectMediaType] = useState<"movie" | "tv">("movie");

  const { data: candidates = [], isLoading, isError, isFetching } =
    useTmdbCandidates(workId, searchParams);
  const { mutate: applyMatch, isPending: applying } = useApplyTmdbMatch();

  const selected = candidates.find(
    (c) => c.tmdb_id === selectedId && c.media_type === selectedMediaType
  ) ?? null;

  function handleSearch() {
    setSelectedId(null);
    setSearchParams({
      queryOverride: draftQuery.trim() || null,
      mediaTypeHint: draftMediaType || null,
    });
  }

  function handleKeyDown(e: React.KeyboardEvent) {
    if (e.key === "Enter") handleSearch();
  }

  function handleApply() {
    if (!selected) return;
    applyMatch(
      { workId, tmdbId: selected.tmdb_id, mediaType: selected.media_type, lock: false },
      { onSuccess: onClose }
    );
  }

  function handleApplyDirectId() {
    const id = parseInt(directIdInput, 10);
    if (isNaN(id) || id <= 0) return;
    applyMatch(
      { workId, tmdbId: id, mediaType: directMediaType, lock: false },
      { onSuccess: onClose }
    );
  }

  const mediaTypeTabs: { value: "" | "movie" | "tv"; label: string }[] = [
    { value: "", label: "全て" },
    { value: "movie", label: "映画" },
    { value: "tv", label: "TV" },
  ];

  return (
    <div className="fixed inset-0 bg-black/70 flex items-center justify-center z-50 p-4">
      <div className="bg-surface-elevated border border-subtle rounded-lg w-full max-w-2xl max-h-[92vh] flex flex-col shadow-2xl">

        {/* ── Header ── */}
        <div className="flex items-center justify-between px-5 py-4 border-b border-subtle flex-shrink-0">
          <div>
            <h2 className="text-sm font-semibold text-gray-100">TMDb 候補を選択</h2>
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

        {/* ── locked バナー ── */}
        {isLocked && (
          <div className="mx-4 mt-3 px-3 py-2 bg-yellow-900/30 border border-yellow-700/50 rounded text-xs text-yellow-400 flex-shrink-0">
            🔒 固定済みです。候補を適用すると固定が解除され「手動照合」になります。
          </div>
        )}

        {/* ── Search bar ── */}
        <div className="px-4 pt-3 pb-2 border-b border-subtle flex-shrink-0 flex flex-col gap-2">
          <div className="flex gap-2">
            <input
              type="text"
              value={draftQuery}
              onChange={(e) => setDraftQuery(e.target.value)}
              onKeyDown={handleKeyDown}
              placeholder="検索語…"
              className="flex-1 bg-surface border border-subtle rounded px-3 py-1.5 text-sm text-gray-200 placeholder-gray-600 outline-none focus:border-mantis-600 transition-colors"
            />
            <button
              onClick={handleSearch}
              disabled={isFetching}
              className="px-3 py-1.5 text-xs bg-mantis-700 hover:bg-mantis-600 text-white rounded transition-colors disabled:opacity-50"
            >
              {isFetching ? "…" : "検索"}
            </button>
          </div>

          {/* Media type tabs */}
          <div className="flex gap-1">
            {mediaTypeTabs.map((tab) => (
              <button
                key={tab.value}
                onClick={() => setDraftMediaType(tab.value)}
                className={clsx(
                  "px-3 py-1 text-xs rounded transition-colors",
                  draftMediaType === tab.value
                    ? "bg-mantis-700/60 text-mantis-300 border border-mantis-600"
                    : "text-gray-500 hover:text-gray-300 border border-transparent"
                )}
              >
                {tab.label}
              </button>
            ))}
          </div>
        </div>

        {/* ── Candidate list ── */}
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
              <p className="text-xs mt-1 text-gray-700">検索語を変更して再検索してください</p>
            </div>
          ) : (
            <div className="flex flex-col gap-2">
              {candidates.map((c) => (
                <CandidateCard
                  key={`${c.tmdb_id}-${c.media_type}`}
                  c={c}
                  selected={selectedId === c.tmdb_id && selectedMediaType === c.media_type}
                  onClick={() => {
                    if (selectedId === c.tmdb_id && selectedMediaType === c.media_type) {
                      setSelectedId(null);
                      setSelectedMediaType("");
                    } else {
                      setSelectedId(c.tmdb_id);
                      setSelectedMediaType(c.media_type);
                    }
                  }}
                />
              ))}
            </div>
          )}
        </div>

        {/* ── 直接 ID 入力 ── */}
        <div className="flex-shrink-0 border-t border-subtle">
          <button
            onClick={() => setShowDirectId((v) => !v)}
            className="w-full px-5 py-2 text-xs text-gray-600 hover:text-gray-400 text-left transition-colors flex items-center gap-1"
          >
            <span>{showDirectId ? "▾" : "▸"}</span>
            TMDb ID で直接指定
          </button>

          {showDirectId && (
            <div className="px-5 pb-3 flex items-center gap-2">
              <input
                type="number"
                value={directIdInput}
                onChange={(e) => setDirectIdInput(e.target.value)}
                placeholder="TMDb ID"
                min={1}
                className="w-28 bg-surface border border-subtle rounded px-2 py-1 text-sm text-gray-200 placeholder-gray-600 outline-none focus:border-mantis-600"
              />
              <select
                value={directMediaType}
                onChange={(e) => setDirectMediaType(e.target.value as "movie" | "tv")}
                className="bg-surface border border-subtle rounded px-2 py-1 text-xs text-gray-300 outline-none focus:border-mantis-600"
              >
                <option value="movie">映画</option>
                <option value="tv">TV</option>
              </select>
              <button
                onClick={handleApplyDirectId}
                disabled={!directIdInput || applying}
                className="px-3 py-1 text-xs bg-blue-700 hover:bg-blue-600 text-white rounded transition-colors disabled:opacity-40"
              >
                適用
              </button>
            </div>
          )}
        </div>

        {/* ── Footer ── */}
        <div className="flex items-center justify-end px-5 py-3 border-t border-subtle flex-shrink-0">
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
