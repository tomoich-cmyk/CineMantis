import { clsx } from "clsx";
import { useLibraryStore } from "@/store/libraryStore";
import { FilterBar, useActiveFilterCount } from "@/components/layout/FilterBar";

export function TopBar() {
  const {
    activeSection,
    filters,
    setFilter,
    filterBarOpen,
    toggleFilterBar,
    isSelectMode,
    selectedWorkIds,
    toggleSelectMode,
  } = useLibraryStore();

  const activeFilterCount = useActiveFilterCount();
  const sectionLabel = activeSection === "all-drama" ? "DRAMA" : "MOVIE";

  return (
    <div className="flex flex-col flex-shrink-0 border-b border-black bg-[#050505]">
      <div className="h-10 flex items-center gap-3 px-3 text-xs text-gray-400">
        <div className="flex items-center gap-2 w-48 flex-shrink-0">
          <span className="h-4 w-4 rounded-full bg-mantis-500" />
          <span className="text-gray-100 font-semibold">CineMantis</span>
          <span className="text-gray-600">{sectionLabel}</span>
        </div>

        <input
          type="search"
          value={filters.query}
          onChange={(e) => setFilter("query", e.target.value)}
          placeholder={`${sectionLabel} を検索`}
          className="h-7 w-full max-w-xl bg-[#101014] border border-[#24242a] px-3 text-sm text-gray-200 placeholder-gray-600 outline-none focus:border-mantis-500"
        />

        <div className="flex-1" />

        <button
          onClick={toggleFilterBar}
          className={clsx(
            "relative h-7 px-3 border border-[#24242a] text-gray-500 hover:text-gray-100 hover:bg-[#121212]",
            filterBarOpen && "bg-mantis-900/30 text-mantis-300 border-mantis-700/60",
          )}
        >
          絞込
          {activeFilterCount > 0 && (
            <span className="absolute -top-2 -right-2 min-w-4 h-4 px-1 rounded-full bg-mantis-500 text-[10px] text-black font-bold">
              {activeFilterCount}
            </span>
          )}
        </button>

        <button
          onClick={toggleSelectMode}
          className={clsx(
            "h-7 px-3 border border-[#24242a] text-gray-500 hover:text-gray-100 hover:bg-[#121212]",
            isSelectMode && "bg-blue-900/30 text-blue-300 border-blue-700/60",
          )}
        >
          選択{selectedWorkIds.length > 0 ? ` ${selectedWorkIds.length}` : ""}
        </button>
      </div>

      {filterBarOpen && <FilterBar />}
    </div>
  );
}
