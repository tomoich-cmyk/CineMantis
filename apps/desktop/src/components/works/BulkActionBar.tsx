import { useState } from "react";
import { clsx } from "clsx";
import { useLibraryStore } from "@/store/libraryStore";
import { useTags } from "@/hooks/useTags";
import {
  useBulkSetWatchStatus,
  useBulkSetFavorite,
  useBulkAddTag,
  useBulkRemoveTag,
  useBulkSetMatchStatus,
} from "@/hooks/useBulk";

type WatchStatus = "unwatched" | "watching" | "watched" | "skipped";

// ─────────────────────────────────────────────────────────────────────────────

export function BulkActionBar() {
  const { selectedWorkIds, clearSelection, toggleSelectMode } =
    useLibraryStore();
  const count = selectedWorkIds.length;

  const { data: allTags = [] } = useTags();
  const { mutate: setWatchStatus, isPending: settingStatus } = useBulkSetWatchStatus();
  const { mutate: setFavorite,    isPending: settingFav }    = useBulkSetFavorite();
  const { mutate: addTag,         isPending: addingTag }     = useBulkAddTag();
  const { mutate: removeTag,      isPending: removingTag }   = useBulkRemoveTag();
  const { mutate: setMatchStatus, isPending: settingMatch }  = useBulkSetMatchStatus();

  const [tagMenuOpen, setTagMenuOpen]       = useState(false);
  const [matchMenuOpen, setMatchMenuOpen]   = useState(false);

  const isPending = settingStatus || settingFav || addingTag || removingTag || settingMatch;

  const WATCH_BTNS: { value: WatchStatus; label: string }[] = [
    { value: "unwatched", label: "未視聴" },
    { value: "watching",  label: "視聴中" },
    { value: "watched",   label: "視聴済" },
    { value: "skipped",   label: "スキップ" },
  ];

  function fireWatchStatus(status: WatchStatus) {
    if (!count) return;
    setWatchStatus({ workIds: selectedWorkIds, status });
  }

  function fireFavorite(on: boolean) {
    if (!count) return;
    setFavorite({ workIds: selectedWorkIds, isFavorite: on });
  }

  function fireAddTag(tagId: number) {
    if (!count) return;
    addTag({ workIds: selectedWorkIds, tagId });
    setTagMenuOpen(false);
  }

  function fireRemoveTag(tagId: number) {
    if (!count) return;
    removeTag({ workIds: selectedWorkIds, tagId });
    setTagMenuOpen(false);
  }

  function fireMatchStatus(status: "locked" | "manual" | "unmatched") {
    if (!count) return;
    setMatchStatus({ workIds: selectedWorkIds, status });
    setMatchMenuOpen(false);
  }

  return (
    <div className="px-4 py-2 border-b border-subtle bg-blue-950/30 flex items-center gap-2 flex-wrap text-xs">

      {/* カウント + 全選択 / 解除 */}
      <span className="text-blue-300 font-medium min-w-[60px]">
        {count > 0 ? `${count}件選択` : "未選択"}
      </span>
      <div className="flex gap-1">
        <button
          onClick={clearSelection}
          className="px-2 py-0.5 border border-blue-800/60 rounded text-blue-500 hover:text-blue-300 transition-colors"
        >
          解除
        </button>
        <button
          onClick={toggleSelectMode}
          className="px-2 py-0.5 border border-subtle rounded text-gray-600 hover:text-gray-400 transition-colors"
        >
          選択終了
        </button>
      </div>

      <div className="w-px h-4 bg-gray-700 mx-1" />

      {/* 視聴状態 */}
      <div className="flex gap-1 items-center">
        <span className="text-gray-600">状態:</span>
        {WATCH_BTNS.map((b) => (
          <button
            key={b.value}
            disabled={!count || isPending}
            onClick={() => fireWatchStatus(b.value)}
            className="px-2 py-0.5 border border-gray-700 rounded text-gray-400 hover:text-gray-100 hover:border-gray-500 transition-colors disabled:opacity-30"
          >
            {b.label}
          </button>
        ))}
      </div>

      <div className="w-px h-4 bg-gray-700 mx-1" />

      {/* お気に入り */}
      <div className="flex gap-1 items-center">
        <button
          disabled={!count || isPending}
          onClick={() => fireFavorite(true)}
          className="px-2 py-0.5 border border-yellow-900/60 rounded text-yellow-600 hover:text-yellow-400 hover:border-yellow-700 transition-colors disabled:opacity-30"
          title="お気に入りON"
        >
          ★ ON
        </button>
        <button
          disabled={!count || isPending}
          onClick={() => fireFavorite(false)}
          className="px-2 py-0.5 border border-gray-700 rounded text-gray-500 hover:text-gray-300 transition-colors disabled:opacity-30"
          title="お気に入りOFF"
        >
          ★ OFF
        </button>
      </div>

      {/* タグ */}
      {allTags.length > 0 && (
        <>
          <div className="w-px h-4 bg-gray-700 mx-1" />
          <div className="relative">
            <button
              disabled={!count || isPending}
              onClick={() => { setTagMenuOpen((o) => !o); setMatchMenuOpen(false); }}
              className={clsx(
                "px-2 py-0.5 border rounded transition-colors disabled:opacity-30",
                tagMenuOpen
                  ? "bg-mantis-900/40 border-mantis-700/60 text-mantis-300"
                  : "border-gray-700 text-gray-400 hover:text-gray-200"
              )}
            >
              タグ ▾
            </button>
            {tagMenuOpen && (
              <div className="absolute top-full left-0 mt-1 z-50 bg-surface-elevated border border-subtle rounded shadow-xl min-w-[160px] py-1">
                <div className="px-2 py-1 text-[10px] text-gray-600 uppercase tracking-wider">追加</div>
                {allTags.map((tag) => (
                  <button
                    key={`add-${tag.id}`}
                    onClick={() => fireAddTag(tag.id)}
                    className="w-full text-left px-3 py-1 text-xs text-gray-300 hover:bg-surface-hover transition-colors"
                  >
                    + {tag.name}
                  </button>
                ))}
                <div className="border-t border-subtle my-1" />
                <div className="px-2 py-1 text-[10px] text-gray-600 uppercase tracking-wider">削除</div>
                {allTags.map((tag) => (
                  <button
                    key={`rm-${tag.id}`}
                    onClick={() => fireRemoveTag(tag.id)}
                    className="w-full text-left px-3 py-1 text-xs text-gray-500 hover:bg-surface-hover transition-colors"
                  >
                    − {tag.name}
                  </button>
                ))}
              </div>
            )}
          </div>
        </>
      )}

      {/* 照合操作 */}
      <div className="w-px h-4 bg-gray-700 mx-1" />
      <div className="relative">
        <button
          disabled={!count || isPending}
          onClick={() => { setMatchMenuOpen((o) => !o); setTagMenuOpen(false); }}
          className={clsx(
            "px-2 py-0.5 border rounded transition-colors disabled:opacity-30",
            matchMenuOpen
              ? "bg-surface-hover border-gray-500 text-gray-200"
              : "border-gray-700 text-gray-500 hover:text-gray-300"
          )}
        >
          照合 ▾
        </button>
        {matchMenuOpen && (
          <div className="absolute top-full left-0 mt-1 z-50 bg-surface-elevated border border-subtle rounded shadow-xl min-w-[140px] py-1">
            <button
              onClick={() => fireMatchStatus("locked")}
              className="w-full text-left px-3 py-1.5 text-xs text-yellow-500 hover:bg-surface-hover transition-colors"
            >
              🔒 ロック
            </button>
            <button
              onClick={() => fireMatchStatus("manual")}
              className="w-full text-left px-3 py-1.5 text-xs text-blue-400 hover:bg-surface-hover transition-colors"
            >
              ✏ 手動照合に変更
            </button>
            <div className="border-t border-subtle my-1" />
            <button
              onClick={() => fireMatchStatus("unmatched")}
              className="w-full text-left px-3 py-1.5 text-xs text-red-500 hover:bg-surface-hover transition-colors"
            >
              × 照合を解除
            </button>
          </div>
        )}
      </div>

      {/* ローディングインジケータ */}
      {isPending && (
        <span className="text-gray-600 text-[11px] animate-pulse ml-2">処理中…</span>
      )}
    </div>
  );
}
