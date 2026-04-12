import { clsx } from "clsx";
import { useLibraryStore } from "@/store/libraryStore";
import type { SortField } from "@cinemantis/shared-types";

const SORT_OPTIONS: { value: SortField; label: string }[] = [
  { value: "title", label: "タイトル" },
  { value: "year", label: "製作年" },
  { value: "user_rating", label: "自分の評価" },
  { value: "external_rating", label: "外部評価" },
  { value: "play_count", label: "視聴回数" },
  { value: "last_played_at", label: "最終視聴" },
  { value: "created_at", label: "追加日" },
  { value: "runtime_sec", label: "再生時間" },
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
  } = useLibraryStore();

  return (
    <div className="flex flex-col border-b border-subtle bg-surface-elevated flex-shrink-0">
      {/* 上段: 検索・ソート・表示切替 */}
      <div className="flex items-center gap-2 px-4 py-2">
        {/* Search */}
        <input
          type="search"
          placeholder="タイトル・人物・タグ…"
          value={filters.query}
          onChange={(e) => setFilter("query", e.target.value)}
          className="flex-1 max-w-xs bg-surface border border-subtle rounded px-3 py-1 text-sm text-gray-200 placeholder-gray-600 outline-none focus:border-mantis-600 transition-colors"
        />

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
    </div>
  );
}
