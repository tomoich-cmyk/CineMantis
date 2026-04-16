import { clsx } from "clsx";
import { useLibraryStore } from "@/store/libraryStore";
import { FilterBar, useActiveFilterCount } from "@/components/layout/FilterBar";
import type { SortField } from "@cinemantis/shared-types";
import type { Density } from "@/store/libraryStore";

const DENSITY_OPTIONS: { value: Density; label: string; title: string }[] = [
  { value: "compact",  label: "小", title: "コンパクト表示" },
  { value: "normal",   label: "中", title: "標準表示" },
  { value: "relaxed",  label: "大", title: "ゆったり表示" },
];


const SORT_OPTIONS: { value: SortField; label: string }[] = [
  { value: "title",          label: "タイトル" },
  { value: "year",           label: "製作年" },
  { value: "user_rating",    label: "自分の評価" },
  { value: "external_rating", label: "外部評価" },
  { value: "play_count",     label: "視聴回数" },
  { value: "last_played_at", label: "最終視聴" },
  { value: "created_at",     label: "追加日" },
  { value: "runtime_sec",    label: "再生時間" },
];

const MATCH_FILTER_OPTIONS: { value: string | null; label: string }[] = [
  { value: null,        label: "すべて" },
  { value: "unmatched", label: "未照合" },
  { value: "auto",      label: "自動" },
  { value: "manual",    label: "手動" },
  { value: "locked",    label: "固定" },
];

export function TopBar() {
  const {
    filters,
    setFilter,
    sortField,
    sortOrder,
    setSortField,
    setSortOrder,
    viewMode,
    setViewMode,
    density,
    setDensity,
    filterBarOpen,
    toggleFilterBar,
    isSelectMode,
    selectedWorkIds,
    toggleSelectMode,
  } = useLibraryStore();

  const activeFilterCount = useActiveFilterCount();

  return (
    <div className="flex flex-col border-b border-subtle bg-surface-elevated flex-shrink-0">
      {/* 上段: 検索・フィルタ・ソート・表示切替 */}
      <div className="flex items-center gap-2 px-4 py-2">
        {/* Search */}
        <input
          type="search"
          placeholder="タイトル検索…"
          value={filters.query}
          onChange={(e) => setFilter("query", e.target.value)}
          className="flex-1 max-w-xs bg-surface border border-subtle rounded px-3 py-1 text-sm text-gray-200 placeholder-gray-600 outline-none focus:border-mantis-600 transition-colors"
        />

        {/* FilterBar トグル */}
        <button
          onClick={toggleFilterBar}
          className={clsx(
            "relative flex items-center gap-1.5 px-2.5 py-1 text-xs rounded border transition-colors",
            filterBarOpen
              ? "bg-mantis-900/40 border-mantis-700/60 text-mantis-300"
              : "border-subtle text-gray-500 hover:text-gray-300"
          )}
          title="詳細フィルタ"
        >
          <span>⚙</span>
          <span>絞込</span>
          {activeFilterCount > 0 && (
            <span className="absolute -top-1.5 -right-1.5 min-w-[16px] h-4 px-1 text-[10px] font-bold bg-mantis-600 text-white rounded-full flex items-center justify-center leading-none">
              {activeFilterCount}
            </span>
          )}
        </button>

        <div className="flex-1" />

        {/* Sort */}
        <div className="flex items-center gap-1.5 text-sm">
          <span className="text-gray-500">並び:</span>
          <select
            value={sortField}
            onChange={(e) => setSortField(e.target.value as SortField)}
            className="bg-surface border border-subtle rounded px-2 py-1 text-gray-300 text-sm outline-none focus:border-mantis-600"
          >
            {SORT_OPTIONS.map((o) => (
              <option key={o.value} value={o.value}>
                {o.label}
              </option>
            ))}
          </select>
          <button
            onClick={() => setSortOrder(sortOrder === "asc" ? "desc" : "asc")}
            className="px-2 py-1 text-gray-400 hover:text-gray-100 border border-subtle rounded transition-colors"
            title={sortOrder === "asc" ? "昇順" : "降順"}
          >
            {sortOrder === "asc" ? "↑" : "↓"}
          </button>
        </div>

        {/* 選択モード */}
        <button
          onClick={toggleSelectMode}
          className={clsx(
            "relative flex items-center gap-1 px-2.5 py-1 text-xs rounded border transition-colors",
            isSelectMode
              ? "bg-blue-900/40 border-blue-700/60 text-blue-300"
              : "border-subtle text-gray-500 hover:text-gray-300"
          )}
          title="一括選択モード"
        >
          <span>✓</span>
          <span>選択</span>
          {isSelectMode && selectedWorkIds.length > 0 && (
            <span className="absolute -top-1.5 -right-1.5 min-w-[16px] h-4 px-1 text-[10px] font-bold bg-blue-600 text-white rounded-full flex items-center justify-center leading-none">
              {selectedWorkIds.length}
            </span>
          )}
        </button>

        {/* Keyboard shortcut hint */}
        <div className="relative group">
          <button
            className="px-2 py-1 text-xs text-gray-700 hover:text-gray-400 transition-colors border border-transparent hover:border-subtle rounded"
            tabIndex={-1}
            aria-label="キーボードショートカット一覧"
          >
            ⌨
          </button>
          {/* ツールチップ */}
          <div className="absolute right-0 top-full mt-1 z-30 hidden group-hover:block
                          bg-surface-elevated border border-subtle rounded shadow-xl p-3 w-52 text-[11px]">
            <p className="text-gray-400 font-semibold mb-2">キーボードショートカット</p>
            <table className="w-full border-collapse">
              <tbody className="divide-y divide-subtle">
                {[
                  ["←  →",   "前後の作品に移動"],
                  ["Space",  "選択中の作品を再生"],
                  ["/",      "検索欄にフォーカス"],
                  ["Escape", "詳細パネルを閉じる"],
                ].map(([key, desc]) => (
                  <tr key={key}>
                    <td className="py-1 pr-3 font-mono text-gray-300 whitespace-nowrap">{key}</td>
                    <td className="py-1 text-gray-500">{desc}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>

        {/* Density */}
        <div className="flex border border-subtle rounded overflow-hidden">
          {DENSITY_OPTIONS.map((opt, i) => (
            <button
              key={opt.value}
              onClick={() => setDensity(opt.value)}
              title={opt.title}
              className={clsx(
                "px-2 py-1 text-xs transition-colors",
                i > 0 && "border-l border-subtle",
                density === opt.value
                  ? "bg-mantis-700/40 text-mantis-300 font-bold"
                  : "text-gray-500 hover:text-gray-300"
              )}
            >
              {opt.label}
            </button>
          ))}
        </div>

        {/* View mode */}
        <div className="flex border border-subtle rounded overflow-hidden">
          <button
            onClick={() => setViewMode("grid")}
            className={`px-2 py-1 text-sm transition-colors ${
              viewMode === "grid" ? "bg-mantis-700/40 text-mantis-300" : "text-gray-500 hover:text-gray-300"
            }`}
            title="グリッド表示"
          >
            ⊞
          </button>
          <button
            onClick={() => setViewMode("list")}
            className={`px-2 py-1 text-sm border-l border-subtle transition-colors ${
              viewMode === "list" ? "bg-mantis-700/40 text-mantis-300" : "text-gray-500 hover:text-gray-300"
            }`}
            title="リスト表示"
          >
            ≡
          </button>
        </div>
      </div>

      {/* 下段: 照合状態フィルタ */}
      <div className="flex items-center gap-1.5 px-4 py-1.5">
        <span className="text-xs text-gray-600 mr-1">照合:</span>
        {MATCH_FILTER_OPTIONS.map((opt) => (
          <button
            key={String(opt.value)}
            onClick={() => setFilter("matchStatus", opt.value)}
            className={clsx(
              "px-2.5 py-0.5 text-xs rounded-full transition-colors border",
              filters.matchStatus === opt.value
                ? opt.value === null
                  ? "bg-surface-hover border-gray-500 text-gray-200"
                  : opt.value === "unmatched"
                  ? "bg-red-900/30 border-red-700/60 text-red-400"
                  : opt.value === "locked"
                  ? "bg-yellow-900/30 border-yellow-700/60 text-yellow-400"
                  : "bg-mantis-900/40 border-mantis-700/60 text-mantis-400"
                : "border-transparent text-gray-600 hover:text-gray-400"
            )}
          >
            {opt.label}
          </button>
        ))}
      </div>

      {/* FilterBar（展開時のみ表示） */}
      {filterBarOpen && <FilterBar />}
    </div>
  );
}
