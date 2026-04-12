import { useState } from "react";
import { useLibraryStore } from "@/store/libraryStore";
import { useWorkDetail, useWorkTags, useUpdateStats } from "@/hooks/useWorks";
import { StarRating } from "@/components/common/StarRating";
import { TagBadge } from "@/components/common/TagBadge";

function toAssetUrl(path: string): string {
  const normalized = path.replace(/\\/g, "/");
  return `asset://localhost/${normalized.replace(/^\//, "")}`;
}

function WorkImage({ src, alt }: { src: string | null; alt: string }) {
  const [failed, setFailed] = useState(false);
  if (!src || failed) {
    return (
      <div className="w-full h-full flex items-center justify-center bg-surface">
        <span className="text-5xl opacity-20">🎬</span>
      </div>
    );
  }
  return (
    <img
      src={toAssetUrl(src)}
      alt={alt}
      className="w-full h-full object-cover rounded"
      onError={() => setFailed(true)}
    />
  );
}

function formatRuntime(sec: number | null) {
  if (!sec) return null;
  const h = Math.floor(sec / 3600);
  const m = Math.floor((sec % 3600) / 60);
  return h > 0 ? `${h}時間${m}分` : `${m}分`;
}

const WATCH_STATUS_OPTIONS = [
  { value: "unwatched", label: "未視聴" },
  { value: "watching", label: "視聴中" },
  { value: "watched", label: "視聴済" },
  { value: "skipped", label: "スキップ" },
];

export function DetailPane() {
  const { selectedWorkId, setSelectedWorkId } = useLibraryStore();
  const { data: work, isLoading } = useWorkDetail(selectedWorkId);
  const { data: tags = [] } = useWorkTags(selectedWorkId);
  const { mutate: updateStats } = useUpdateStats();

  if (!selectedWorkId) return null;

  if (isLoading) {
    return (
      <aside className="w-72 flex-shrink-0 flex flex-col border-l border-subtle bg-surface-elevated items-center justify-center">
        <span className="text-gray-600 animate-pulse text-sm">読み込み中…</span>
      </aside>
    );
  }

  if (!work) return null;

  return (
    <aside className="w-72 flex-shrink-0 flex flex-col border-l border-subtle bg-surface-elevated overflow-y-auto">
      {/* Header */}
      <div className="flex items-center justify-between px-4 py-3 border-b border-subtle flex-shrink-0">
        <span className="text-xs text-gray-400 font-medium uppercase tracking-wider">詳細</span>
        <button
          onClick={() => setSelectedWorkId(null)}
          className="text-gray-600 hover:text-gray-300 text-lg leading-none transition-colors"
        >
          ×
        </button>
      </div>

      {/* Poster / Thumbnail */}
      <div className="mx-4 mt-4 aspect-[2/3] bg-surface rounded overflow-hidden flex-shrink-0">
        {/* ポスター優先、なければサムネイル、なければプレースホルダー */}
        <WorkImage
          src={work.poster_path ?? work.thumb_path}
          alt={work.title}
        />
      </div>

      {/* Title / year / runtime */}
      <div className="px-4 py-3 flex flex-col gap-1">
        <h2 className="text-base font-semibold text-gray-100 leading-snug">{work.title}</h2>
        {work.original_title && work.original_title !== work.title && (
          <p className="text-xs text-gray-500 italic">{work.original_title}</p>
        )}
        <div className="flex items-center gap-2 text-xs text-gray-500 flex-wrap">
          {work.year && <span>{work.year}</span>}
          {work.runtime_sec && <><span>·</span><span>{formatRuntime(work.runtime_sec)}</span></>}
          {work.external_rating && (
            <><span>·</span><span className="text-yellow-500">★ {work.external_rating.toFixed(1)}</span></>
          )}
        </div>
        {/* Genres */}
        {work.genres.length > 0 && (
          <div className="flex flex-wrap gap-1 mt-1">
            {work.genres.map((g) => (
              <span key={g} className="text-xs px-1.5 py-0.5 bg-surface rounded text-gray-500">
                {g}
              </span>
            ))}
          </div>
        )}
      </div>

      {/* User stats */}
      <div className="px-4 py-2 border-t border-subtle">
        <div className="flex flex-col gap-2.5">
          {/* Rating */}
          <div className="flex items-center justify-between">
            <span className="text-xs text-gray-500">マイ評価</span>
            <StarRating
              value={work.user_rating}
              size="sm"
              onChange={(v) =>
                updateStats({ work_id: work.id, user_rating: v })
              }
            />
          </div>

          {/* Watch status */}
          <div className="flex items-center justify-between">
            <span className="text-xs text-gray-500">視聴状態</span>
            <select
              value={work.watch_status}
              onChange={(e) =>
                updateStats({ work_id: work.id, watch_status: e.target.value })
              }
              className="bg-surface border border-subtle rounded px-2 py-0.5 text-xs text-gray-300 outline-none focus:border-mantis-600"
            >
              {WATCH_STATUS_OPTIONS.map((o) => (
                <option key={o.value} value={o.value}>
                  {o.label}
                </option>
              ))}
            </select>
          </div>

          {/* Favorite */}
          <div className="flex items-center justify-between">
            <span className="text-xs text-gray-500">お気に入り</span>
            <button
              onClick={() =>
                updateStats({ work_id: work.id, is_favorite: !work.is_favorite })
              }
              className={`text-lg transition-transform hover:scale-110 ${
                work.is_favorite ? "text-yellow-400" : "text-gray-700"
              }`}
            >
              ★
            </button>
          </div>

          {/* Play count */}
          <div className="flex items-center justify-between">
            <span className="text-xs text-gray-500">視聴回数</span>
            <span className="text-xs text-gray-400">{work.play_count} 回</span>
          </div>
          {work.last_played_at && (
            <div className="flex items-center justify-between">
              <span className="text-xs text-gray-500">最終視聴</span>
              <span className="text-xs text-gray-500">
                {new Date(work.last_played_at).toLocaleDateString("ja-JP")}
              </span>
            </div>
          )}
        </div>
      </div>

      {/* Tags */}
      {tags.length > 0 && (
        <div className="px-4 py-2 border-t border-subtle">
          <div className="flex flex-wrap gap-1">
            {tags.map((tag) => (
              <TagBadge key={tag.id} tag={tag} />
            ))}
          </div>
        </div>
      )}

      {/* Synopsis */}
      {work.synopsis && (
        <div className="px-4 py-3 border-t border-subtle">
          <p className="text-xs text-gray-400 leading-relaxed">{work.synopsis}</p>
        </div>
      )}

      {/* Personal note */}
      <div className="px-4 py-3 border-t border-subtle">
        <textarea
          placeholder="メモ…"
          value={work.personal_note ?? ""}
          onChange={(e) =>
            updateStats({ work_id: work.id, personal_note: e.target.value })
          }
          rows={3}
          className="w-full bg-surface border border-subtle rounded px-2 py-1.5 text-xs text-gray-300 placeholder-gray-700 outline-none focus:border-mantis-600 resize-none"
        />
      </div>

      {/* Footer: match status */}
      <div className="px-4 py-2 border-t border-subtle mt-auto flex-shrink-0">
        <div className="flex items-center justify-between text-xs">
          <span className="text-gray-600">照合</span>
          <span
            className={
              work.match_status === "matched" || work.match_status === "locked"
                ? "text-mantis-500"
                : work.match_status === "pending"
                ? "text-yellow-400"
                : "text-gray-600"
            }
          >
            {
              { matched: "照合済", locked: "固定", pending: "照合中", unmatched: "未照合" }[
                work.match_status
              ]
            }
          </span>
        </div>
      </div>
    </aside>
  );
}
