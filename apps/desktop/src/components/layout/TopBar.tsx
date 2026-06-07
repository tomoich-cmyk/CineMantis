import { clsx } from "clsx";
import { useLibraryStore } from "@/store/libraryStore";
import { FilterBar, useActiveFilterCount } from "@/components/layout/FilterBar";

const MATCH_FILTER_OPTIONS: { value: string | null; label: string }[] = [
  { value: null, label: "すべて" },
  { value: "unmatched", label: "未照合" },
  { value: "auto", label: "自動" },
  { value: "manual", label: "手動" },
  { value: "locked", label: "固定" },
];

const PRESET_BUTTONS = [
  { label: "未視聴", key: "watchStatus", value: "unwatched" },
  { label: "視聴中", key: "watchStatus", value: "watching" },
  { label: "高評価", key: "minUserRating", value: 4 },
  { label: "未整理", key: "unorganizedOnly", value: true },
] as const;

export function TopBar() {
  const {
    filters,
    setFilter,
    filterBarOpen,
    toggleFilterBar,
    isSelectMode,
    selectedWorkIds,
    toggleSelectMode,
  } = useLibraryStore();

  const activeFilterCount = useActiveFilterCount();

  return (
    <div className="flex flex-col flex-shrink-0 border-b border-black bg-[#070707]">
      <div className="h-8 flex items-center px-3 gap-3 text-xs text-gray-400 border-b border-[#181818]">
        <div className="flex items-center gap-2 min-w-44">
          <span className="h-4 w-4 rounded-full bg-mantis-500/80" />
          <span className="text-gray-100 font-semibold">CineMantis</span>
          <span className="text-gray-600">▼</span>
        </div>
        <button className="text-mantis-400 text-lg leading-none">←</button>
        <button className="text-gray-700 text-lg leading-none">→</button>
        <div className="flex-1 flex items-center justify-center gap-8 text-gray-600">
          <span>動画</span>
          <span>ライブラリ</span>
          <span>MusicBee View</span>
        </div>
        <input
          type="search"
          value={filters.query}
          onChange={(e) => setFilter("query", e.target.value)}
          placeholder="検索"
          className="w-56 bg-[#111] border border-[#252525] px-2 py-0.5 text-xs text-gray-200 outline-none focus:border-mantis-500"
        />
      </div>

      <div className="h-14 flex items-center gap-5 px-5 bg-gradient-to-r from-[#213849] via-[#1f4b67] to-[#123349] text-gray-100">
        <div className="flex items-center gap-5 text-2xl text-gray-300">
          <button title="前へ" className="hover:text-white">‹</button>
          <button title="再生" className="hover:text-white">▷</button>
          <button title="次へ" className="hover:text-white">›</button>
        </div>
        <div className="flex items-center gap-2 text-gray-300">
          <span className="text-lg">▸</span>
          <div className="h-1 w-24 bg-white/25">
            <div className="h-full w-1/3 bg-white/70" />
          </div>
        </div>
        <div className="w-28 text-center text-lg tracking-wide text-gray-200">★★★★☆</div>
        <div className="flex-1 min-w-0 text-center">
          <div className="truncate text-sm font-medium">CineMantis Library</div>
          <div className="truncate text-[11px] text-gray-300/75">SQLite を正本にした動画ライブラリ管理</div>
          <div className="mt-2 h-1 bg-white/25">
            <div className="h-full w-2/5 bg-white/70" />
          </div>
        </div>
        <div className="text-xs text-gray-200 font-mono">0:07 / 4:09</div>
        <div className="flex items-center gap-2 text-gray-200">
          <button
            onClick={toggleFilterBar}
            className={clsx(
              "relative px-2 py-1 border border-white/20 hover:bg-white/10",
              filterBarOpen && "bg-white/10 text-white",
            )}
          >
            ⚙
            {activeFilterCount > 0 && (
              <span className="absolute -top-2 -right-2 min-w-4 h-4 px-1 rounded-full bg-mantis-500 text-[10px] text-black font-bold">
                {activeFilterCount}
              </span>
            )}
          </button>
          <button
            onClick={toggleSelectMode}
            className={clsx("px-2 py-1 border border-white/20 hover:bg-white/10", isSelectMode && "bg-blue-400/20")}
          >
            選択{selectedWorkIds.length > 0 ? ` ${selectedWorkIds.length}` : ""}
          </button>
        </div>
      </div>

      <div className="h-8 flex items-center gap-2 px-3 bg-[#0b0b0b] border-t border-white/5">
        <span className="text-xs text-gray-600 mr-1">プリセット</span>
        {PRESET_BUTTONS.map((preset) => {
          const active = filters[preset.key] === preset.value;
          return (
            <button
              key={preset.label}
              onClick={() => setFilter(preset.key, active ? (preset.key === "unorganizedOnly" ? false : null) as never : preset.value as never)}
              className={clsx(
                "px-2 py-0.5 text-xs border border-transparent text-gray-500 hover:text-gray-200",
                active && "bg-mantis-600 text-black border-mantis-500",
              )}
            >
              {preset.label}
            </button>
          );
        })}
        <div className="h-4 w-px bg-[#2a2a2a] mx-1" />
        {MATCH_FILTER_OPTIONS.map((opt) => (
          <button
            key={String(opt.value)}
            onClick={() => setFilter("matchStatus", opt.value)}
            className={clsx(
              "px-2 py-0.5 text-xs border border-transparent text-gray-600 hover:text-gray-300",
              filters.matchStatus === opt.value && "text-mantis-300 border-mantis-700/60 bg-mantis-900/20",
            )}
          >
            {opt.label}
          </button>
        ))}
      </div>

      {filterBarOpen && <FilterBar />}
    </div>
  );
}
