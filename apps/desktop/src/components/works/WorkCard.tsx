import { useState } from "react";
import { clsx } from "clsx";
import type { WorkSummary } from "@cinemantis/shared-types";
import { StarRating } from "@/components/common/StarRating";

interface Props {
  work: WorkSummary;
  selected: boolean;
  onSelect: () => void;
}

/** サムネイル画像。読み込み失敗時はプレースホルダーに切り替わる */
function Thumbnail({ src, alt }: { src: string | null; alt: string }) {
  const [failed, setFailed] = useState(false);

  if (!src || failed) {
    return (
      <div className="w-full h-full flex flex-col items-center justify-center gap-1 bg-gradient-to-b from-surface-hover to-surface">
        <span className="text-3xl opacity-20">🎬</span>
      </div>
    );
  }

  return (
    <img
      // Tauri ではローカルパスに "asset://" プロトコルが必要
      // 通常の file:// は Content Security Policy でブロックされるため変換
      src={toAssetUrl(src)}
      alt={alt}
      className="w-full h-full object-cover"
      onError={() => setFailed(true)}
      loading="lazy"
    />
  );
}

/** ローカルパスを Tauri の asset:// URL に変換 */
function toAssetUrl(path: string): string {
  // Windows: C:\foo\bar → asset://localhost/C:/foo/bar
  // unix:    /foo/bar   → asset://localhost/foo/bar
  const normalized = path.replace(/\\/g, "/");
  return `asset://localhost/${normalized.replace(/^\//, "")}`;
}

export function WorkCard({ work, selected, onSelect }: Props) {
  return (
    <div
      onClick={onSelect}
      className={clsx(
        "group flex flex-col cursor-pointer rounded overflow-hidden border transition-all select-none",
        selected
          ? "border-mantis-500 ring-1 ring-mantis-500/40 shadow-lg shadow-mantis-900/30"
          : "border-surface-border hover:border-gray-600"
      )}
    >
      {/* Poster / Thumbnail */}
      <div className="aspect-[2/3] bg-surface-hover flex items-center justify-center relative overflow-hidden">
        <Thumbnail src={work.posterPath ?? null} alt={work.title} />

        {/* Overlay badges */}
        {work.isFavorite && (
          <span className="absolute top-1.5 right-1.5 text-xs drop-shadow">⭐</span>
        )}

        {/* Unwatched indicator: bottom edge bar */}
        {work.watchStatus === "unwatched" && (
          <div className="absolute bottom-0 left-0 right-0 h-0.5 bg-mantis-500/70" />
        )}

        {/* Watching: progress indicator */}
        {work.watchStatus === "watching" && (
          <div className="absolute bottom-0 left-0 right-0 h-0.5 bg-blue-500/70" />
        )}

        {/* Unmatched warning */}
        {work.matchStatus === "unmatched" && (
          <span
            className="absolute top-1.5 left-1.5 text-[10px] px-1 rounded bg-black/60 text-yellow-400"
            title="外部情報未照合"
          >
            未照合
          </span>
        )}
      </div>

      {/* Info */}
      <div className="px-2 py-1.5 bg-surface-elevated flex flex-col gap-0.5">
        <p className="text-xs font-medium text-gray-200 line-clamp-2 leading-tight">
          {work.title}
        </p>
        <div className="flex items-center justify-between">
          <span className="text-[11px] text-gray-500">{work.year ?? "—"}</span>
          {work.userRating !== null ? (
            <StarRating value={work.userRating} size="xs" readonly />
          ) : (
            work.externalRating !== null && (
              <span className="text-[11px] text-yellow-600">
                ★ {work.externalRating.toFixed(1)}
              </span>
            )
          )}
        </div>
      </div>
    </div>
  );
}
