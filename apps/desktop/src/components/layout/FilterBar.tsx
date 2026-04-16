import { clsx } from "clsx";
import { useLibraryStore } from "@/store/libraryStore";
import { useFilterOptions } from "@/hooks/useWorks";
import { useTags } from "@/hooks/useTags";

// ── ヘルパー ──────────────────────────────────────────────────────────────────

/** アクティブな拡張フィルタ数（TopBar バッジ用にエクスポート） */
export function useActiveFilterCount(): number {
  const { filters } = useLibraryStore();
  let count = 0;
  if (filters.yearFrom !== null)      count++;
  if (filters.yearTo   !== null)      count++;
  if (filters.genre    !== null)      count++;
  if (filters.country  !== null)      count++;
  if (filters.minUserRating !== null) count++;
  if (filters.isFavorite)             count++;
  if (filters.tagIds.length > 0)      count++;
  return count;
}

// ── コンポーネント ────────────────────────────────────────────────────────────

export function FilterBar() {
  const { filters, setFilter } = useLibraryStore();
  const { data: opts } = useFilterOptions();
  const { data: allTags = [] } = useTags();

  const activeCount = useActiveFilterCount();

  return (
    <div className="px-4 py-3 border-b border-subtle bg-surface flex flex-col gap-3">
      {/* 行1: ジャンル / 国 / 年代 */}
      <div className="flex items-center gap-3 flex-wrap">

        {/* ジャンル */}
        <div className="flex items-center gap-1.5">
          <span className="text-xs text-gray-500 whitespace-nowrap">ジャンル</span>
          <select
            value={filters.genre ?? ""}
            onChange={(e) => setFilter("genre", e.target.value || null)}
            className="bg-surface-elevated border border-subtle rounded px-2 py-0.5 text-xs text-gray-300 outline-none focus:border-mantis-600 min-w-[120px]"
          >
            <option value="">すべて</option>
            {opts?.genres.map((g) => (
              <option key={g} value={g}>{g}</option>
            ))}
          </select>
        </div>

        {/* 国 */}
        <div className="flex items-center gap-1.5">
          <span className="text-xs text-gray-500 whitespace-nowrap">製作国</span>
          <select
            value={filters.country ?? ""}
            onChange={(e) => setFilter("country", e.target.value || null)}
            className="bg-surface-elevated border border-subtle rounded px-2 py-0.5 text-xs text-gray-300 outline-none focus:border-mantis-600 min-w-[80px]"
          >
            <option value="">すべて</option>
            {opts?.countries.map((c) => (
              <option key={c} value={c}>{c}</option>
            ))}
          </select>
        </div>

        {/* 年代 */}
        <div className="flex items-center gap-1.5">
          <span className="text-xs text-gray-500 whitespace-nowrap">年代</span>
          <input
            type="number"
            placeholder={opts?.year_min?.toString() ?? "—"}
            value={filters.yearFrom ?? ""}
            onChange={(e) => setFilter("yearFrom", e.target.value ? Number(e.target.value) : null)}
            className="w-16 bg-surface-elevated border border-subtle rounded px-2 py-0.5 text-xs text-gray-300 outline-none focus:border-mantis-600 text-center"
          />
          <span className="text-gray-600 text-xs">〜</span>
          <input
            type="number"
            placeholder={opts?.year_max?.toString() ?? "—"}
            value={filters.yearTo ?? ""}
            onChange={(e) => setFilter("yearTo", e.target.value ? Number(e.target.value) : null)}
            className="w-16 bg-surface-elevated border border-subtle rounded px-2 py-0.5 text-xs text-gray-300 outline-none focus:border-mantis-600 text-center"
          />
        </div>
      </div>

      {/* 行2: 評価 / お気に入り / タグ / リセット */}
      <div className="flex items-center gap-3 flex-wrap">

        {/* 最低評価 */}
        <div className="flex items-center gap-1.5">
          <span className="text-xs text-gray-500 whitespace-nowrap">評価</span>
          <div className="flex gap-0.5">
            {[1, 2, 3, 4, 5].map((n) => (
              <button
                key={n}
                onClick={() =>
                  setFilter("minUserRating", filters.minUserRating === n ? null : n)
                }
                className={clsx(
                  "text-base leading-none transition-colors",
                  (filters.minUserRating ?? 0) >= n
                    ? "text-yellow-400"
                    : "text-gray-700 hover:text-gray-500"
                )}
                title={`${n}点以上`}
              >
                ★
              </button>
            ))}
            {filters.minUserRating !== null && (
              <span className="text-xs text-gray-500 ml-1">以上</span>
            )}
          </div>
        </div>

        {/* お気に入り */}
        <button
          onClick={() => setFilter("isFavorite", !filters.isFavorite)}
          className={clsx(
            "flex items-center gap-1 px-2.5 py-0.5 text-xs rounded-full border transition-colors",
            filters.isFavorite
              ? "bg-yellow-900/30 border-yellow-700/60 text-yellow-400"
              : "border-subtle text-gray-500 hover:text-gray-300"
          )}
        >
          ★ お気に入り
        </button>

        {/* タグ */}
        {allTags.length > 0 && (
          <div className="flex items-center gap-1.5 flex-wrap">
            <span className="text-xs text-gray-500 whitespace-nowrap">タグ</span>
            <div className="flex gap-1 flex-wrap">
              {allTags.map((tag) => {
                const active = filters.tagIds.includes(tag.id);
                return (
                  <button
                    key={tag.id}
                    onClick={() => {
                      const next = active
                        ? filters.tagIds.filter((id) => id !== tag.id)
                        : [...filters.tagIds, tag.id];
                      setFilter("tagIds", next);
                    }}
                    className={clsx(
                      "px-2 py-0.5 text-xs rounded border transition-colors",
                      active
                        ? "bg-mantis-900/40 border-mantis-700/60 text-mantis-400"
                        : "border-subtle text-gray-600 hover:text-gray-300"
                    )}
                  >
                    {tag.name}
                  </button>
                );
              })}
            </div>
          </div>
        )}

        <div className="flex-1" />

        {/* リセット */}
        {activeCount > 0 && (
          <button
            onClick={() => {
              setFilter("yearFrom", null);
              setFilter("yearTo", null);
              setFilter("genre", null);
              setFilter("country", null);
              setFilter("minUserRating", null);
              setFilter("isFavorite", false);
              setFilter("tagIds", []);
            }}
            className="text-xs text-gray-600 hover:text-gray-300 transition-colors underline-offset-2 hover:underline"
          >
            フィルタをリセット
          </button>
        )}
      </div>
    </div>
  );
}
