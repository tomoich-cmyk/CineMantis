import { useState } from "react";
import { clsx } from "clsx";
import { convertFileSrc } from "@tauri-apps/api/core";
import type { WorkSummary } from "@cinemantis/shared-types";
import { StarRating } from "@/components/common/StarRating";
import { useOpenWorkFile } from "@/hooks/useWatch";
import { useLibraryStore } from "@/store/libraryStore";
import type { Density } from "@/store/libraryStore";

// 密度 → Info エリアのクラス
const INFO_CLASSES: Record<Density, { wrap: string; title: string; meta: string }> = {
  compact: { wrap: "px-1.5 py-1",   title: "text-[10px]", meta: "text-[10px]" },
  normal:  { wrap: "px-2 py-1.5",   title: "text-xs",     meta: "text-[11px]" },
  relaxed: { wrap: "px-2.5 py-2",   title: "text-sm",     meta: "text-xs"     },
};

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
      src={toAssetUrl(src)}
      alt={alt}
      className="w-full h-full object-cover"
      onError={() => setFailed(true)}
      loading="lazy"
    />
  );
}

/** ローカルパスを Tauri の asset URL に変換（Tauri v2 公式 API 使用） */
function toAssetUrl(path: string): string {
  return convertFileSrc(path);
}

/** 秒数を "h:mm:ss" 形式にフォーマット */
function formatPosition(sec: number): string {
  const h = Math.floor(sec / 3600);
  const m = Math.floor((sec % 3600) / 60);
  const s = Math.floor(sec % 60);
  if (h > 0) return `${h}:${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}`;
  return `${m}:${String(s).padStart(2, "0")}`;
}

export function WorkCard({ work, selected, onSelect }: Props) {
  const { mutate: openFile, isPending: opening } = useOpenWorkFile();
  const { isSelectMode, selectedWorkIds, toggleSelectWork, density } = useLibraryStore();
  const info = INFO_CLASSES[density];

  const isChecked = selectedWorkIds.includes(work.id);

  // 再生進捗: resume_position_sec / runtime_sec (0〜1)
  const progress =
    work.resumePositionSec !== null && work.runtimeSec !== null && work.runtimeSec > 0
      ? Math.min(work.resumePositionSec / work.runtimeSec, 1)
      : null;

  function handlePlay(e: React.MouseEvent) {
    e.stopPropagation();
    openFile(work.id);
  }

  function handleClick() {
    if (isSelectMode) {
      toggleSelectWork(work.id);
    } else {
      onSelect();
    }
  }

  return (
    <div
      onClick={handleClick}
      className={clsx(
        "group flex flex-col cursor-pointer rounded overflow-hidden border transition-all select-none",
        isSelectMode && isChecked
          ? "border-blue-500 ring-1 ring-blue-500/40 shadow-lg shadow-blue-900/20"
          : selected
          ? "border-mantis-500 ring-1 ring-mantis-500/40 shadow-lg shadow-mantis-900/30"
          : "border-surface-border hover:border-gray-600"
      )}
    >
      {/* Poster / Thumbnail */}
      <div className="aspect-[2/3] bg-surface-hover flex items-center justify-center relative overflow-hidden">
        <Thumbnail src={work.posterPath ?? null} alt={work.title} />

        {/* ▶ 再生ボタン（ホバー時に表示） */}
        <button
          onClick={handlePlay}
          disabled={opening}
          className="absolute inset-0 flex items-center justify-center
                     opacity-0 group-hover:opacity-100 transition-opacity
                     bg-black/30"
          title="再生"
        >
          <span
            className={clsx(
              "w-10 h-10 flex items-center justify-center rounded-full bg-black/70 text-white text-lg transition-transform",
              opening ? "opacity-50" : "hover:scale-110"
            )}
          >
            {opening ? "…" : "▶"}
          </span>
        </button>

        {/* 選択モード: チェックボックス */}
        {isSelectMode && (
          <div className={clsx(
            "absolute top-1.5 left-1.5 w-5 h-5 rounded border-2 flex items-center justify-center pointer-events-none transition-colors",
            isChecked
              ? "bg-blue-600 border-blue-600 text-white"
              : "bg-black/50 border-gray-400"
          )}>
            {isChecked && <span className="text-[11px] leading-none">✓</span>}
          </div>
        )}

        {/* Overlay badges */}
        {work.isFavorite && (
          <span className="absolute top-1.5 right-1.5 text-xs drop-shadow pointer-events-none">⭐</span>
        )}

        {/* 視聴進捗バー（resumePositionSec がある場合は詳細なプログレス） */}
        {progress !== null ? (
          <div className="absolute bottom-0 left-0 right-0 h-1 bg-black/40 pointer-events-none">
            <div
              className="h-full bg-blue-500 transition-all"
              style={{ width: `${(progress * 100).toFixed(1)}%` }}
            />
          </div>
        ) : work.watchStatus === "watching" ? (
          <div className="absolute bottom-0 left-0 right-0 h-0.5 bg-blue-500/70 pointer-events-none" />
        ) : work.watchStatus === "unwatched" ? (
          <div className="absolute bottom-0 left-0 right-0 h-0.5 bg-mantis-500/70 pointer-events-none" />
        ) : null}

        {/* Unmatched warning */}
        {work.matchStatus === "unmatched" && (
          <span
            className="absolute top-1.5 left-1.5 text-[10px] px-1 rounded bg-black/60 text-yellow-400 pointer-events-none"
            title="外部情報未照合"
          >
            未照合
          </span>
        )}

        {/* 再開位置ラベル（視聴中 + 進捗あり） */}
        {work.watchStatus === "watching" && work.resumePositionSec !== null && (
          <span className="absolute bottom-2 right-1.5 text-[10px] bg-black/70 text-blue-300 px-1 rounded pointer-events-none">
            {formatPosition(work.resumePositionSec)}
          </span>
        )}
      </div>

      {/* Info */}
      <div className={clsx("bg-surface-elevated flex flex-col gap-0.5", info.wrap)}>
        <p className={clsx("font-medium text-gray-200 line-clamp-2 leading-tight", info.title)}>
          {work.title}
        </p>
        <div className="flex items-center justify-between">
          <span className={clsx("text-gray-500", info.meta)}>{work.year ?? "—"}</span>
          {/* compact では評価表示を省略してタイトル優先 */}
          {density !== "compact" && (
            work.userRating !== null ? (
              <StarRating value={work.userRating} size="xs" readonly />
            ) : (
              work.externalRating !== null && (
                <span className={clsx("text-yellow-600", info.meta)}>
                  ★ {work.externalRating.toFixed(1)}
                </span>
              )
            )
          )}
        </div>
      </div>
    </div>
  );
}
